# CalibRaw

[![License: GPL-3.0-or-later](https://img.shields.io/badge/License-GPL--3.0--or--later-blue.svg)](COPYING)
[![Language: Rust](https://img.shields.io/badge/Language-Rust_1.92+-orange.svg)](https://www.rust-lang.org/)
[![Graphics: wgpu](https://img.shields.io/badge/Graphics-wgpu_%2F_WGSL-red.svg)](https://wgpu.rs/)
[![Platform](https://img.shields.io/badge/Platform-Linux_%7C_Android_%7C_Windows_%7C_macOS-green.svg)](https://github.com/Duecki1/CalibRaw/releases)

CalibRaw is a fast, non-destructive, GPU-accelerated RAW photo editor. Built from the ground up in Rust using `wgpu` and custom WGSL compute shaders, it provides an approachable Lightroom-style workflow with flexible color tools, advanced demosaicing, and on-device AI masking.

CalibRaw runs natively on **Linux**, **Android**, **Windows**, and **macOS** with zero Electron or browser runtime layers.

[**Download Releases**](https://github.com/Duecki1/CalibRaw/releases) • [**Supported Formats**](#supported-raw-formats) • [**Building**](#building-from-source)

---

## Showcase

<img width="100%" alt="CalibRaw Main Interface" src="https://github.com/user-attachments/assets/69d52204-05e4-4c40-8a74-f9200ec01e5b" />

<p>
  <img width="49.5%" alt="CalibRaw Masking and Color Tools" src="https://github.com/user-attachments/assets/aa0e8a4d-5772-47ee-8938-c782b1597a0d" />
  <img width="49.5%" alt="CalibRaw Darkroom View" src="https://github.com/user-attachments/assets/789540d1-f6b6-4466-8db4-48340c3f2ed7" />
</p>

---

## Key Features

- **Pure Native Rust & GPU Compute:** Built with `egui` and `wgpu`. Fast startup, lightweight memory profile, and direct compute shader rendering.
- **Color Grading & Point Color:** Sample colors directly with an eyedropper, fine-tune custom Hue, Saturation, and Luminance ranges, and preview affected areas with a live selection mask.
- **Advanced Demosaicing:** Shader-based Bayer RCD, Fujifilm X-Trans (Markesteijn 3-pass), and Dual Demosaicing to reduce noise and artifacts in high-ISO images.
- **Sigmoid Tone Mapping:** Smooth highlight roll-off inspired by modern scene-referred color workflows, paired with Opposed and LCh highlight recovery.
- **Local AI Masking (Offline):** Optional, locally-run ONNX models for subject selection (BiRefNet), object segmentation (Meta SAM 2.1), healing (LaMa), and AI denoise.
- **Native Android App:** A real ARM64 NDK build with 16 KB memory page support, touch layouts, and MediaStore exporting.
- **Non-Destructive Edits:** All sliders, curves, and masks are saved to lightweight `.calibraw` JSON sidecar files.
- **Tiled Export & CLI:** Exports large RAW files in tiles to avoid GPU memory limits, plus a command-line tool for batch rendering.

---

## Tooling & Workflow

### Color & Tone
- **Point Color:** Sample a color in global or mask HSL controls, refine its Hue/Saturation/Luminance span, preview the selected range, and apply targeted shifts.
- **8-Band Color Mixer:** Adjust Hue, Saturation, and Luminance across 8 individual color bands.
- **3-Way Color Grading:** Dedicated Shadows, Midtones, Highlights, and Global color wheels mapped through Oklab color space.
- **Tone Curves:** Luminance curve and individual Red, Green, and Blue splines with monotonic limits.
- **Sigmoid Tone Mapping:** Provides smooth, film-like highlight compression without harsh clipping.

### Demosaicing & Corrections
- **Bayer RCD:** Directional interpolation for standard Bayer sensors.
- **Fujifilm X-Trans:** Directional derivative and homogeneity demarcation pipeline for Fuji sensors.
- **Dual Demosaicing:** Blends edge-preserving and smoothed passes to suppress chroma noise at high ISOs.
- **Lens Correction:** Automatic distortion, chromatic aberration, and vignette correction via the Lensfun database.
- **HDR Merge:** Combine multiple bracketed RAW exposures into one extended-range image.

### Masking & AI
- **Masks:** Brush, Linear Gradient, Radial Gradient, and Shape masks with independent curves and adjustments.
- **Local AI Tools:** One-click subject masking (BiRefNet), click-to-select object masking (Meta SAM 2.1), and healing (LaMa + Laplace solver). Runs entirely offline on your own hardware.
- **GPU Creative Effects:** Stackable effect cards for masks or the full image, including Light Rays, Lens Blur (bokeh), Motion Blur, Radial Blur, Fog, Smoke, Glow, and Neon.

---

## Supported RAW Formats

CalibRaw uses **LibRaw (0.22.1)** for broad camera compatibility, paired with **Rawler (0.8.0)** for DNG and JPEG-XL DNG streams:

```text
.3fr   .ari   .arw   .bay   .bmq   .cap   .cine  .cr2   .cr3   .crw
.cs1   .dc2   .dcr   .dcs   .dng   .drf   .eip   .erf   .fff   .gpr
.iiq   .k25   .kc2   .kdc   .mdc   .mef   .mos   .mrw   .nef   .nrw
.obm   .orf   .pef   .ptx   .pxn   .qtk   .r3d   .raf   .raw   .rdc
.rw2   .rwl   .rwz   .sr2   .srf   .srw   .sti   .tif   .tiff  .x3f
```

---

## Architecture

CalibRaw is structured as a modular Rust workspace:

```text
CalibRaw/
├── crates/
│   ├── calibraw-core/  # Color transforms, decoders (LibRaw/Rawler), metadata, sidecars
│   ├── calibraw-gpu/   # wgpu pipelines, WGSL compute shaders, tiled renderer
│   ├── calibraw-ai/    # ONNX Runtime models (BiRefNet, SAM 2.1, LaMa, RawNIND)
│   ├── calibraw-ui/    # egui/eframe interface for desktop and touch
│   ├── calibraw-cli/   # Headless batch export CLI tool
│   └── calibraw-ffi/   # Android JNI and NativeActivity bridge
└── android/            # Android Gradle packaging
```

### Headless CLI Export
Batch rendering can be run directly from the terminal:

```sh
calibraw-develop-export --input photo.ARW --output photo.jpg
```

---

## Building from Source

### Prerequisites
- **Rust Toolchain:** 1.92.0 or newer
- **System Dependencies:** `libclang`, `cmake`, `pkg-config`, and standard C/C++ build tools (for LibRaw and Lensfun)

### Desktop (Linux / macOS / Windows)
```sh
git clone https://github.com/Duecki1/CalibRaw.git
cd CalibRaw
cargo run -p calibraw-ui --bin calibraw --release
```

### Android (APK)
```sh
rustup target add aarch64-linux-android
cargo install cargo-ndk --version 4.1.2 --locked
./gradlew assembleDebug
```

---

## Acknowledgments

CalibRaw's shader logic and tooling draw valuable inspiration from the open-source imaging community:

- **[darktable](https://www.darktable.org/)** – For pioneering open-source imaging algorithms, including Bayer RCD, X-Trans Markesteijn, Opposed highlight recovery, and Sigmoid tone mapping logic.
- **[GIMP](https://www.gimp.org/) & [Ansel](https://ansel.photos/)** – For Laplace inpainting foundations and color normalization references.
- **[LibRaw](https://github.com/LibRaw/LibRaw), [Rawler](https://github.com/dnglab/dnglab), & [Lensfun](https://github.com/lensfun/lensfun)** – For RAW decoding, DNG metadata extraction, and lens profile databases.
- **[RapidRAW](https://github.com/CyberSys/RapidRAW)** – For interface and workflow layout inspiration that sparked the creation of this project.

*See [Third-Party Notices](THIRD_PARTY_NOTICES.md) for complete licensing, attributions, and model sources.*

---

## License

CalibRaw is free software licensed under the **GNU General Public License v3.0 or later** ([GPL-3.0-or-later](COPYING)).

*Adobe and Lightroom are registered trademarks of Adobe Inc. CalibRaw is an independent open-source project and is not affiliated with, sponsored by, or endorsed by Adobe Inc.*
