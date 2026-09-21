# CalibRaw
CalibRaw is a fast, GPU-accelerated RAW photo editor. <br>
It is designed for responsive performance and non-destructive editing.

CalibRaw provides an approachable, open-source RAW editing workflow. <br>
CalibRaw is available for Linux, Android, Windows, and macOS, with Linux and Android being the primary focus.

Download the latest release [here](https://github.com/Duecki1/CalibRaw/releases).

## Showcase
<img width="100%" alt="image" src="https://github.com/user-attachments/assets/69d52204-05e4-4c40-8a74-f9200ec01e5b" />

<p>
  <img width="49.5%" alt="image" src="https://github.com/user-attachments/assets/aa0e8a4d-5772-47ee-8938-c782b1597a0d" />
  <img width="49.5%" alt="image" src="https://github.com/user-attachments/assets/789540d1-f6b6-4466-8db4-48340c3f2ed7" />
</p>

## Key Features

- **GPU-accelerated:** Built with Rust, wgpu, and custom WGSL compute shaders for real-time performance.
- **Non-destructive:** All edits and adjustments are non-destructive and saved to lightweight `.calibraw` sidecar files.
- **Advanced demosaicing:** Features Bayer RCD, Fujifilm X-Trans (Markesteijn), and noise-resilient Dual Demosaicing.
- **Masking:** Supports brush and shape masks with independent tone curves and adjustments.
- **Creative effects:** Mask effects such as light rays, lens blur, motion blur, fog, glow, and more.
- **Optional AI:** Local Subject & Object masks, [AI Denoise](https://github.com/darktable-org/darktable-ai), and AI object removal.
- **Multi-platform:** Native builds for Linux, Android, Windows, and macOS.
- **HDR merge:** Select 2 or more images to HDR merge them.
- **Automatic lens correction:** Uses the Lensfun database.
- **RAW compatibility fallback:** LibRaw remains the primary camera RAW backend;
  Rawler handles selected DNG variants when it provides better compatibility.
  See the [RAW compatibility notes](docs/raw-compatibility.md).

## Supported RAW Formats

CalibRaw currently recognizes the following RAW file extensions:

```text
.3fr   .ari   .arw   .bay   .bmq   .cap   .cine  .cr2   .cr3   .crw
.cs1   .dc2   .dcr   .dcs   .dng   .drf   .eip   .erf   .fff   .gpr
.iiq   .k25   .kc2   .kdc   .mdc   .mef   .mos   .mrw   .nef   .nrw
.obm   .orf   .pef   .ptx   .pxn   .qtk   .r3d   .raf   .raw   .rdc
.rw2   .rwl   .rwz   .sr2   .srf   .srw   .sti   .tif   .tiff  .x3f
```

## Contributing
Contributions are welcome. Please open a pull request or create an issue.

### Roadmap / To-Do
- [x] Bundle ONNX Runtime
- [ ] Gather community feedback
- [x] DNG Support

## Special Thanks

- **[darktable](https://www.darktable.org/)** – for their exceptional contributions to open-source color science and raw processing algorithms.
- **[RapidRAW](https://github.com/CyberSys/RapidRAW)** – for workflow and interface inspiration that sparked the creation of this project.
- **[GIMP](https://www.gimp.org/) & [Ansel](https://ansel.photos/)** – for image editing algorithms and foundations.
- **[LibRaw](https://github.com/LibRaw/LibRaw) & [Lensfun](https://github.com/lensfun/lensfun)** – for the underlying decoding and lens-correction backends.
- **[Rawler](https://github.com/dnglab/dnglab)** – for selected DNG and RAW
  compatibility paths.

*Check [Third-Party Notices](THIRD_PARTY_LICENSES.md) for a more detailed list.*

## AI Notice

This project was developed with the assistance of LLMs.

CalibRaw also supports optional, locally run AI models for smart masking, denoising, and object removal.

## License

CalibRaw is GPL-3.0-or-later. See [COPYING](COPYING), [NOTICE.md](NOTICE.md), and
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). Complete resolved Rust
dependency, font, and icon terms are in
[THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md).
