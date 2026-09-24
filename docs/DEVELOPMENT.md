# Development

Use Rust 1.92 with LibRaw, Lensfun, libclang, and the platform graphics
dependencies.

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- \
  -D warnings -W clippy::perf -W clippy::large_stack_arrays \
  -W clippy::redundant_clone -W unreachable-pub
cargo deny check
cargo run -p calibraw-ui --bin calibraw --release
```

The CPU brush-raster baseline is a harness-free benchmark (kept independent
of the test suite): `cargo bench -p calibraw-core --bench mask_rasterization`.
It reports throughput for positive and erase dabs on a fixed 512x512 raster;
the benchmark does not alter production rasterization or numerical behavior.

## UI conventions

`crates/calibraw-ui/src/ui/theme.rs` is the shared entry point for UI styling.
Use its control heights, spacing, cards, toolbar rows, and widget helpers before
adding screen-specific styling. Keep specialized image canvases, mask cards,
and color controls in their existing components.

- Use secondary/primary action buttons for forms and settings, `menu_item` for
  regular menu actions, and `context_menu_item` for selectable navigation menus.
  Preserve each menu's explicit `ui.close()` behavior.
- Use `icons` helpers for icon actions and folder disclosure controls. Conditional
  variants delegate to the same control so enabled state does not change sizing.
- Use the shared form rows, text edits, and combo builders. The adjustment slider
  owns value editing, reset, focus, and pointer/scroll handling on both desktop
  and touch layouts.
- Use `dialog_window`, dialog action rows, and keyboard helpers for existing
  window dialogs. Keep initial focus requests one-time, and run keyboard fallback
  after controls process input. Modal surfaces use themed egui frames; preserve
  their existing dismissal and backdrop policies.
- Route user tab navigation through `activate_tab`, and sidebar/tool changes
  through `AppAction`. Background document loading and batch operations contain
  documented exceptions: interactive tab activation can cancel their AI work.
- Keep serialized preference names stable when changing UI labels or helpers.

Run `cargo test -p calibraw-ui --lib --locked` for headless UI regressions.
The ignored `portrait_gpu_layout_and_input` test additionally checks rendered
preview geometry and pointer/touch behavior; it needs a GPU adapter and an
isolated `XDG_CONFIG_HOME`. `CALIBRAW_PREVIEW_TEST_SCREENSHOT` optionally captures
its rendered fixture. Review desktop and Android layouts in all four themes
when a change affects appearance.

## Diagnostics and release helpers

`scripts/generate_licenses.sh` is the canonical reproducible wrapper around
cargo-about. It normalizes generated line endings and writes the ignored
`THIRD_PARTY_LICENSES.md` bundle used by release packaging. The script pins
cargo-about to 0.9.2; run `bash scripts/generate_licenses.sh` to generate the
bundle locally.

`tools/colorchecker_wb_validate.py` compares rendered and reference D50 XYZ
ColorChecker patches using CIEDE2000. Run `python3 tools/colorchecker_wb_validate.py
--self-check` for the implementation check, or pass a CSV with the required
`patch`, `reference_x/y/z`, and `rendered_x/y/z` columns. `--json PATH`
additionally writes machine-readable results.

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

The app supports 16 KB pages. Verify a built APK with:

```sh
cargo xtask verify-android-16kb android/app/build/outputs/apk/debug/app-debug.apk
```

### GitHub release signing

Only direct pushes to `main` in the **Build Linux and Android** workflow build a
signed release APK. Pull requests, pushes to other branches, and manual runs
build a debug APK and do not access the release-signing secrets.

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
are published to `Pictures/CalibRaw` through MediaStore where available.


Long-running operations require the app to remain open. Android 13 and newer
may request notification permission for progress updates.
