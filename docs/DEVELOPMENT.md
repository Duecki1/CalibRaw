# Development

Use Rust 1.92 with LibRaw, Lensfun, libclang, and the platform graphics
dependencies. Crate ownership, dependency rules and threading contracts are in
[ARCHITECTURE.md](ARCHITECTURE.md).

```sh
cargo run -p calibraw-ui --bin calibraw --release
```

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
cargo xtask jni-contract
cargo xtask ui-lint
cargo deny check
```

- `cargo xtask arch-check` enforces the crate dependency rules listed in
  ARCHITECTURE.md against `cargo tree -e normal,build --target all`.
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

- Use secondary/primary action buttons for forms and settings, `menu_item` for
  regular menu actions, and `context_menu_item` for selectable navigation menus.
  Preserve each menu's explicit `ui.close()` behavior.
- Use `icons` helpers for icon actions and folder disclosure controls. Conditional
  variants delegate to the same control so enabled state does not change sizing.
- Use the shared form rows, text edits, `NumberField` and combo builders.
  `moduwu_design::Slider` owns value editing, reset, focus, keyboard and
  accessibility actions, and pointer/scroll handling on desktop and touch
  layouts; `AdjustmentSlider` adapts it to `FloatParamSpec` and adds the
  photographic track gradients. Scroll areas that contain sliders consult
  `moduwu_design::slider_scroll_locked`.
- Pick the control from what the value means: `moduwu_design::toggle` or
  `toggle_with_help` for on/off options, `segmented_button` for one choice among
  a few, `AngleDial` (via `float_param_angle`) for directions, the shared
  feathered range (`components::feathered_range`) for ranges with soft edges
  such as depth, luminance and Point Color, and `pattern_seed` (Shuffle) for
  random pattern seeds.
- Use `dialog_window`, dialog action rows, and keyboard helpers for existing
  window dialogs. Keep initial focus requests one-time, and run keyboard fallback
  after controls process input. Modal surfaces use themed egui frames; preserve
  their existing dismissal and backdrop policies.
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

Run the independent library checks above for reusable design-system regressions
and `cargo test -p calibraw-ui --lib --locked` for headless UI integration regressions.
The ignored `portrait_gpu_layout_and_input` test additionally checks rendered
preview geometry and pointer/touch behavior; it needs a GPU adapter and an
isolated `XDG_CONFIG_HOME`. `CALIBRAW_PREVIEW_TEST_SCREENSHOT` optionally captures
its rendered fixture. Review desktop and Android layouts in all six themes
when a change affects appearance.

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
local measurements, not a cross-platform or Lightroom comparison.

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

Android depth masks and fog share a pinned Depth Anything V2 Small FP32 model
(~99 MB), run at 518 × 518 on CPU with two inference threads and weight
prepacking disabled. Its inverse-depth predictions are converted to the shared
near=0, far=255 convention. Desktop uses Depth Anything 3 Mono Large; its
731 MB artifact has a much larger download and inference memory footprint.

The app supports 16 KB pages. Verify a built APK with:

```sh
cargo xtask verify-android-16kb android/app/build/outputs/apk/debug/app-debug.apk
```

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
