# Preview fidelity

CalibRaw has two deliberately different preview sources.

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
