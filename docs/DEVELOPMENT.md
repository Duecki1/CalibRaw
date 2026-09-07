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

## Diagnostics and release helpers

`scripts/generate_licenses.sh` is the canonical reproducible wrapper around
cargo-about. It normalizes generated line endings before updating
`THIRD_PARTY_LICENSES.md`; CI uses the same wrapper and pins cargo-about to
0.9.2, which should also be used for local regeneration.

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
