# Preview fidelity

CalibRaw has two deliberately different preview sources.

The developed-image revision is independent of pan and zoom. A sharp detail
crop stays on screen while a single background worker prepares the latest
view. Crops include an 8% navigation border in addition to processing support;
only the region inside the processing halo is reused for presentation.
Coverage, physical pixel density, quality, and eligibility for native sensor
processing determine whether a cached crop is sufficient. Cache validation
compares its source sampling density with the newly planned crop, so a texture
at the working-edge cap cannot remain frozen through subsequent zoom steps.
Processing support is calculated from the active adjustments and mask effects;
its border has a separate pixel allowance so it does not reduce visible image
resolution during the first zoom steps. Resizing, rotating,
changing DPI, and changing geometry all reevaluate that decision. A missing
detail crop is rebuilt even when there is no remaining navigation timer.

The fitted fallback also finishes pending edits while zoomed in, so newly
exposed areas cannot retain old adjustments. Obsolete offscreen worker results
are discarded before GPU upload. GPU programs and compatible crop allocations
are reused, and a failed upload backs off before retrying.

Android sizes fitted proxies, detail crops, and native refinements against the
GPU resource plan before allocation. Each graph uses at most half of the
384 MiB process budget, including mask capacity and the safety margin, so the
fitted image and detail crop can coexist. Max quality requests are limited to
the resolution that fits this budget. An incompatible old detail graph is
released once its replacement is ready to upload.

The fitted, navigation, and wide zoom views use `build_region_proxy`. For a
sensor RAW this is an interactive approximation: samples with the same CFA
colour are averaged into a smaller synthetic mosaic. That averaging happens
before highlight reconstruction, demosaic, and denoise, so it can hide an
individually clipped photosite, alias a fine coloured pattern, and lower the
noise variance presented to the denoiser.

After zoom navigation settles, the detail preview switches to an untouched
native-resolution sensor crop when the padded crop fits the bounded native
working edge (4096 px on desktop, 2048 px on Android). The complete processing
graph then runs at native sampling density. Only the developed display result
is sampled to the viewport size. Opposed-chroma highlight reconstruction also
uses statistics calculated from the complete source rather than from the crop.
This is the preview fidelity reference and is the path to use when judging
clipped highlights, fine coloured detail, and shadow noise.

The remaining approximations are explicit:

- A settled crop wider than the native working edge continues to use the CFA
  proxy to keep GPU memory bounded. Zooming farther in makes the crop cross into
  the native path.
- Preview working textures use preview precision, and viewport presentation
  linearly samples the output-encoded GPU texture rather than applying the
  exporter's linear-light Lanczos filter.
- Crop boundaries include the normal processing halo, but very long-range
  effects and full-frame tone statistics can still differ slightly from export.

Final export remains authoritative: it processes native RAW tiles, performs
full-resolution tone analysis, stitches developed linear RGB, and resizes in
linear light before output encoding.

## Viewport behavior

Wheel/trackpad zoom and two-finger pan/zoom keep the image point under the
gesture anchor. Zoom-out extends to 70% of the fitted size to leave room for
mask handles outside the image. Double-click/tap returns to fit from a smaller
or larger view, and switches from fit to native 100% sampling, accounting for
display density. Reset also works in the canvas margins and on mask/Remove
canvases. A remaining finger after a
pinch cannot accidentally start painting or panning.

Portrait Develop uses the complete safe-area width with a width-fitted image.
The top controls and resizable bottom tool sheet overlay that canvas; changing
tools or resizing the sheet does not change image scale. The exposed area
determines pan bounds and gesture ownership, so image edges can be brought
above the sheet and tool interactions cannot start through it. Android system
insets remain outside the canvas. Android landscape uses the same full canvas
with its tool rail and sidebar overlaid on the right.

Regression checks: `cargo test -p calibraw-ui --lib --locked`. The optional
`portrait_gpu_layout_and_input` test exercises the actual GPU texture and egui
layout without a window (a software Vulkan adapter also works). Run it with `-- --ignored`, an isolated
`XDG_CONFIG_HOME`, and optionally `CALIBRAW_PREVIEW_TEST_SCREENSHOT` pointing to
a PNG file to capture the rendered fixture.
