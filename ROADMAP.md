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
- [X]  LUT support
- [X]  Watermark
- [ ]  Retouch tab: blemish removal, skin smoothing, eye enhancement (maybe)
- [ ]  Straight brush lines, rectangle and circle brushes
- [X]  Preset opacity
- [ ]  Copy tone curve
- [ ]  Lens profile saving for unrecognized lenses
- [X]  Rotate in crop tab
- [X]  Depth effects rework: two surfaces per pixel, light bake with shadows, traced Wet Surfaces reflections (docs/rework/DEPTH_EFFECTS_PLAN.md)
- [ ]  Depth lens blur fit: far tree lines read too sharp (about 5 px instead of 30–40 px)
- [ ]  Depth cut-out edges: raise the share of the foreground edge within 1.5 px of the photo's edge above 66%
- [ ]  Fog, Relight: faint ghost edge where the depth outline differs from the photo's (DSC08062, roofline behind the sign)
- [ ]  Wet Surfaces: tune the roughness thresholds on photos (where puddles and sheen may form on gravel and grass)
- [ ]  Rain and Wet Surfaces: offer them in the effect picker again once judged
- [ ]  Depth effects performance pass: working size, shadow face size, filter taps, air light steps
- [ ]  Release notes for the depth rework's look changes (shadow shapes, traced reflections, Rain drop spacing, lights without depth)

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
- [X]  Better mask and sky edges
- [X]  Light rays subtract/intersect fix
- [X]  Undo/redo for object mask and crop
- [X]  Cancelable object mask

## Library

- [X]  Shift/Ctrl multi-select, select all
- [X]  Arrow-key navigation follows sorting and filters
- [X]  Thumbnails use the cropped aspect ratio
- [X]  Narrower sidebar, larger default thumbnails
- [X]  Full screen view
- [X]  Warning instead of hard limit for large images
- [ ]  Virtual copies
- [ ]  Culling tools
- [ ]  Collections
- [X]  Tiff thumbnails
- [X]  Stale thumbnail re-rendering
- [ ]  Faster initial indexing on HDDs, automatic HDD mode
- [X]  Duplicate-file export naming (`_1`, `_2`, …)
- [X]  Keep zoom when switching color profiles
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
- [X]  Lower preview memory use: on-demand effect buffers, shared mask atlas
- [X]  Android notifications, new tablet UI
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
- [X]  Simpler right-click menu and detail card
- [ ]  New app icon
- [X]  Tutorial pop-ups with images/GIFs, smaller GIFs
- [ ]  Keyboard shortcuts changeable
- [ ]  Preset repository
