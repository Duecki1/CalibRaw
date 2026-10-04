<div align="center">

# CalibRaw

**A free, open-source RAW photo editor for Linux, Android, Windows and macOS, with a Lightroom-style workflow and AI tools that run locally.**

[![Latest release](https://img.shields.io/github/v/release/Duecki1/CalibRaw?label=Download&color=brightgreen)](https://github.com/Duecki1/CalibRaw/releases/latest)
[![License: GPL-3.0-or-later](https://img.shields.io/badge/License-GPL--3.0--or--later-blue.svg)](COPYING)
[![Language: Rust](https://img.shields.io/badge/Language-Rust_1.92+-orange.svg)](https://www.rust-lang.org/)
[![Graphics: wgpu](https://img.shields.io/badge/Graphics-wgpu_%2F_WGSL-red.svg)](https://wgpu.rs/)
[![Platform](https://img.shields.io/badge/Platform-Linux_%7C_Android_%7C_Windows_%7C_macOS-green.svg)](https://github.com/Duecki1/CalibRaw/releases/latest)

[**Download**](#download) • [**Why CalibRaw**](#why-calibraw) • [**Features**](#key-features) • [**FAQ**](#faq) • [**Supported Formats**](#supported-formats) • [**Building**](#building-from-source)

</div>

<!-- TODO: hero GIF (10 to 15 s): open a RAW, apply an AI subject mask, finish with a before/after.
     "Export > Create Edit Replay…" produces an MP4 of an edit you can convert to GIF. -->
<img width="100%" alt="CalibRaw editing a RAW photo" src="https://github.com/user-attachments/assets/b4afad3f-8577-411c-a310-995e71a79cb4" />

<p>
  <img width="49.5%" alt="Library" src="https://github.com/user-attachments/assets/c64bbc1e-d2c1-4498-8d64-7ba6ee552af6" />
  <img width="49.5%" alt="Android (Library/Edit/AI Subject)" src="https://github.com/user-attachments/assets/0f6ffbb4-233a-4a58-9386-fbd44393225b" />
</p>

Edit RAW files with sliders, curves, color grading and masks. AI tools can select a subject or the sky, remove objects and denoise high-ISO photos. Everything runs **on your device**, without an account or subscription.

---

## Download

| Platform | Get it |
| --- | --- |
| **Linux** (x86_64 / ARM64) | `.AppImage` from the [latest release](https://github.com/Duecki1/CalibRaw/releases/latest) |
| **Android** (ARM64) | `.apk` from the [latest release](https://github.com/Duecki1/CalibRaw/releases/latest) |
| **Windows** (x86_64 / ARM64) | Installer or portable `.zip` from the [latest release](https://github.com/Duecki1/CalibRaw/releases/latest) |
| **macOS** (Apple Silicon / Intel) | `.zip` from the [latest release](https://github.com/Duecki1/CalibRaw/releases/latest) |

AI models are downloaded once, on first use and only after you agree; they are verified by SHA-256 and then run offline.

---

## Why CalibRaw

- **Lightroom-style controls:** Light, Tone Curve, Color, Color Mixer, Color Grading, Detail, Optics, Effects and Masks panels, with a scene-referred pipeline, Sigmoid tone mapping and highlight recovery.
- **Offline AI:** Subject, sky, depth and click-to-select object masks, AI Remove (Big-LaMa) that blends its fill into your photo's grain, and RawNIND denoise for Bayer and X-Trans RAWs.
- **Native interface and GPU processing:** Written in Rust with `wgpu`, WGSL compute shaders and an `egui` interface.
- **Android support:** The Android app uses the same engine and `.calibraw` sidecars as the desktop app, with layouts for touch controls.
- **Effects with masks:** Apply depth-based fog, smoke, light rays, glow, halation, edge glow, neon and lens, motion, radial or tilt-shift blur to the whole photo or inside a mask.
- **Edit Replay:** Export a 30 FPS MP4 showing your edit step by step for Reels, Shorts or before/after posts.

---

## Key Features

- **Rust & GPU Compute:** Built with `egui` and `wgpu`, with compute shaders for image processing.
- **Color Grading & Point Color:** Sample colors directly with an eyedropper, fine-tune custom Hue, Saturation, and Luminance ranges, and preview affected areas with a live selection mask.
- **Demosaicing:** Shader-based Bayer RCD, Fujifilm X-Trans (Markesteijn 3-pass), and Dual Demosaicing to reduce noise and artifacts in high-ISO images.
- **Sigmoid Tone Mapping:** Smooth highlight roll-off inspired by modern scene-referred color workflows, paired with Opposed and LCh highlight recovery.
- **Local AI Tools (Offline):** Subject, sky, depth and object masks, AI Denoise, and AI Remove.
- **Android App:** An ARM64 NDK build with 16 KB memory page support, touch layouts, and MediaStore exporting.
- **Non-Destructive Edits:** All sliders, curves, and masks are saved to lightweight `.calibraw` JSON sidecar files.
- **Presets:** Save any combination of adjustment groups, masks and profile choices as a named preset, apply it in Develop or to a whole Library selection, and share it as a `.calibraw-preset` file.
- **Tiled Export & CLI:** Exports large RAW files in tiles to avoid GPU memory limits, plus a command-line tool for batch rendering.
- **JPEG XL Export:** Exports lossless `.jxl` images at 8-bit or 16-bit precision, including EXIF metadata when enabled.
- **Edit Replay:** Export a short 30 FPS MP4 that replays the current edit by category.

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
- **AI Masks:** Subject and background (BiRefNet), sky (SkySeg U2Net), depth range (Depth Anything 3 on desktop, Depth Anything V2 on Android), and click-to-select objects (SAM 2.1).
- **AI Remove:** Big-LaMa fills removed objects at native resolution where possible and restores the photo's own grain around the fill.
- **Clone & Heal:** Classic retouching brushes with aligned, fixed and registered source modes.
- **AI Denoise:** RawNIND denoises Bayer RAWs while demosaicing them and X-Trans RAWs in linear RGB.
- **Privacy:** Every model runs on your device. Models are downloaded only after you agree, and verified by SHA-256.

### Creative Effects

Apply effects to the whole photo or stack them on a local mask. Each card keeps its main controls visible, with optional details collapsed. Hide a card to compare, reset its settings, or remove it independently.

- **Blur & Movement:** Soften distractions with Blur, shape bokeh with Lens Blur, add directional trails with Motion Blur, zoom or spin with Radial Blur, or keep a band in focus with Tilt Shift. Feather the mask for a smooth transition.
- **Glow & Light Rays:** Glow spreads colored light from bright pixels selected by the mask. Light Rays uses the mask to shape light shafts. Both can spread beyond the selection. Drag position pads to place a center or source; Precise position also lets you place a ray source beyond the image.
- **Edge Glow & Neon:** Trace image contours with colored outlines and halos. Both effects stay inside the mask; Neon's Original image control retains the photo behind the lines.
- **Fog & Smoke:** Add atmospheric haze or textured plumes. Fog's Scene depth control builds haze with distance; turn it off for an even-distance veil.
- **Grain:** Add photographic texture with adjustable size, roughness, color, and seed. Use a local mask for selective grain or Fullscreen for a film finish.
- **Halation:** Add warm halos around bright highlights, with radius, highlight threshold, and warmth controls. The halo appears only inside the mask, including light from nearby highlights outside it.
- **Vignette:** Darken edges with negative amounts or brighten them with positive amounts. Adjust the falloff, shape, center, and highlight protection. Its center follows the cropped, rotated frame; the other position pads use the full image.
- **Pixelate:** Turn selected areas into a mosaic with adjustable block size.

---

## FAQ

**How is CalibRaw different from darktable or RapidRAW?**
All three are free. darktable offers detailed module-level control and a mature
color pipeline. RapidRAW's Lightroom-style workflow inspired CalibRaw.
CalibRaw uses a native interface and GPU processing, with the same engine on desktop
and Android. It includes offline AI, creative effects such as depth-aware fog and
light rays, and Edit Replay videos.

**Does any of my data leave my device?**
No. Editing, AI masks, AI Remove and AI Denoise all run locally. CalibRaw only goes online to
download AI models or the ONNX Runtime, and only after you agree.

**Which cameras and files are supported?**
Anything LibRaw supports, plus DNG via Rawler, as well as JPEG, PNG, HEIC and TIFF; see [Supported Formats](#supported-formats).

---

## Supported Formats

### RAW

CalibRaw uses **LibRaw (0.22.1)** for broad camera compatibility, paired with **Rawler (0.8.0)** for DNG and JPEG-XL DNG streams:

```text
.3fr   .ari   .arw   .bay   .bmq   .cap   .cine  .cr2   .cr3   .crw
.cs1   .dc2   .dcr   .dcs   .dng   .drf   .eip   .erf   .fff   .gpr
.iiq   .k25   .kc2   .kdc   .mdc   .mef   .mos   .mrw   .nef   .nrw
.obm   .orf   .pef   .ptx   .pxn   .qtk   .r3d   .raf   .raw   .rdc
.rw2   .rwl   .rwz   .sr2   .srf   .srw   .sti   .x3f
```

### Rendered images

JPEG, PNG, HEIC/HEIF and rendered TIFF files are edited with the same tools. They open looking exactly as captured, with embedded ICC profiles, Display P3, HLG/PQ HDR HEIF and EXIF orientation honored:

```text
.jpg   .jpeg  .jpe   .png   .heic  .heif  .hif   .tif   .tiff
```

When a camera saves RAW+JPEG (or RAW+HEIC), the Library shows each pair once, as the RAW. Turn this off under **Sort & filter → RAW + JPEG**.

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

The reusable UI design library, [Moduwu Design](https://github.com/Duecki1/Moduwu),
is fetched automatically by Cargo from a pinned GitHub commit. See
[Development](docs/DEVELOPMENT.md#design-library-setup) for design-library development.

### Headless CLI Export
Batch rendering can be run directly from the terminal:

```sh
calibraw-develop-export --input photo.ARW --output photo.jxl
```

---

## Building from Source

### Prerequisites
- **Rust Toolchain:** 1.92.0 or newer
- **System Dependencies:** `libclang`, `cmake`, `pkg-config`, and standard C/C++ build tools (for LibRaw and Lensfun)

### Desktop (Linux / Windows / macOS)

Cargo automatically downloads Moduwu Design from GitHub; no separate checkout
is required.

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

CalibRaw's shaders and tools draw on work from these projects:

- **[darktable](https://www.darktable.org/)**: Bayer RCD, X-Trans Markesteijn, Opposed highlight recovery, and Sigmoid tone mapping logic.
- **[GIMP](https://www.gimp.org/) & [Ansel](https://ansel.photos/)**: Laplace inpainting foundations and color normalization references.
- **[LibRaw](https://github.com/LibRaw/LibRaw), [Rawler](https://github.com/dnglab/dnglab), & [Lensfun](https://github.com/lensfun/lensfun)**: RAW decoding, DNG metadata extraction, and lens profile databases.
- **[RapidRAW](https://github.com/CyberTimon/RapidRAW)**: Interface and workflow layout inspiration.

*See [Third-Party Notices](THIRD_PARTY_NOTICES.md) for complete licensing, attributions, and model sources.*

---

## License

CalibRaw is free software licensed under the **GNU General Public License v3.0 or later** ([GPL-3.0-or-later](COPYING)).

*Adobe and Lightroom are registered trademarks of Adobe Inc. CalibRaw is an independent open-source project and is not affiliated with, sponsored by, or endorsed by Adobe Inc.*
