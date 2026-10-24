# CalibRaw Roadmap

Checked items are done. Order within a section is not a priority.

## Editing

- [X]  Relighting
- [X]  Halation
- [X]  Point color
- [X]  Auto level
- [X]  HDR merge (clipping highlights, error correction, phone support)
- [X]  Focus stacking
- [X]  Clone and heal fixes
- [X]  Light rays: no steps in sky when source is off-screen
- [X]  Temp and tint slider fixes
- [X]  Separate lens correction toggles: geometry, vignette
- [X]  Lightroom preset import
- [X]  DCP profile import setup
- [X]  Camera profile highlight fixes
- [X]  Export: JPEG XL, single export button, remembered settings
- [X]  Grain module
- [X]  Chromatic aberration and color fringing
- [ ]  Dead and hot pixel correction
- [ ]  Auto tone, auto geometry, auto perspective
- [ ]  Depth lens blur
- [ ]  LUT support
- [ ]  Watermark
- [ ]  Retouch tab: blemish removal, skin smoothing, eye enhancement (maybe)
- [ ]  Straight brush lines, rectangle and circle brushes
- [ ]  Preset opacity, replace/merge apply mode
- [ ]  Copy tone curve
- [ ]  Lens profile saving for unrecognized lenses
- [ ]  Rotate in crop tab

## Masks

- [X]  Mask types as components
- [X]  Freeform path mask (feather, grow)
- [X]  Sky mask
- [X]  Depth mask (experimental)
- [X]  Universal feather and grow
- [X]  Luminance mask performance
- [X]  Movable masks
- [X]  Mask overlay toggle
- [X]  Subject mask: error when nothing is selected, faster on Android
- [X]  Per-element visibility toggle
- [ ]  Compound masks
- [ ]  Better mask and sky edges
- [ ]  Light rays subtract/intersect fix
- [X]  Undo/redo for object mask and crop
- [ ]  Cancelable object mask

## Library

- [X]  Shift/Ctrl multi-select, select all
- [X]  Arrow-key navigation follows sorting and filters
- [X]  Thumbnails use the cropped aspect ratio
- [X]  Narrower sidebar, larger default thumbnails
- [X]  Full screen view
- [X]  Warning instead of hard limit for large images
- [ ]  Virtual copies
- [ ]  Culling tools
- [ ]  Lightroom collections
- [ ]  Tiff thumbnails
- [ ]  Stale thumbnail re-rendering
- [ ]  Faster initial indexing on HDDs, automatic HDD mode
- [ ]  Duplicate-file export naming (`_1`, `_2`, …)
- [ ]  Keep zoom when switching color profiles
- [ ]  Better drag and drop
- [ ]  Edit history
- [ ]  Export progress in export button

## Platform

- [X]  Android: edit replay, tablet and mask-tab layout fixes, open in / share, HDR merge, depth, thumbnail worker
- [X]  Windows GPU fix, macOS icon
- [X]  Paths with `ß ä ö ü`
- [X]  Bundled ONNX runtime, AI model download and update fixes
- [X]  Signed APK builds, GitHub update checker, CUDA restart prompt
- [X]  Error dialogs for all failures (e.g. out of memory)
- [X]  AuRaw → CalibRaw conversion script
- [ ]  Panorama and focus alignment
- [X]  Lower preview memory use: RAM-scaled limit, on-demand mask layers
- [ ]  Lower preview memory use: on-demand effect buffers, shared mask atlas
- [ ]  Android notifications, new tablet UI
- [ ]  AppImage built on oldest Ubuntu LTS
- [ ]  Play Store, Winget
- [ ]  Crash log file
- [X]  Keep AI models out of the cache directory

## UI and usability

- [X]  Undo/redo shortcut fixes
- [X]  Arrow keys on number fields
- [X]  Space bar to pan while dragging
- [X]  Double-click slider reset fixes
- [X]  Grey theme
- [X]  Issue and PR templates
- [ ]  Simpler right-click menu and detail card
- [ ]  New app icon
- [ ]  Tutorial pop-ups with images/GIFs, smaller GIFs
- [ ]  Keyboard shortcuts changeable
- [ ]  Preset repository
