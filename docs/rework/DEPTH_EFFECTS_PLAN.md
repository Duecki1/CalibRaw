# Depth and normals effects: rebuild plan

Status (2026-11-07): P1 to P7 are built; all awaiting the user's review. The old depth
path is deleted (§8's check returns nothing outside this file). Left over: P8 timings,
the P2/P3/P5 photo checks listed in §11, and photos S1 to S3, which the user never
gave. The user dropped all device tests on 2026-11-06 (Tab M11 timings and memory are
out of scope; everything is judged on the desktop). Rain and Wet Surfaces are no longer
offered in the effect picker (2026-11-07, the user's decision to save work: nobody uses
them); edits that hold them keep rendering them through the new path. Written
2026-11-06 against `fb9e74df`, reviewed and corrected the same day (§12); the user
decided the texture limit and the air light the same day (§12, items 3 and 6). Work
through it phase by phase (§6). After each phase, fill in its row in §11 (Progress and
measurements) and stop for the user's review before starting the next phase.

This plan replaces the code behind every effect that reads scene depth or scene
normals: Relight, Fog, Smoke, Rain, Wet Surfaces, the scene lights and shadows, and
Glow/Neon scene emission. It also replaces the depth post-processing in `calibraw-ai`
and the sidecar layout of the depth result. Depth Range masks keep their behaviour.

---

## 1. Rules for this work

AGENTS.md, docs/ARCHITECTURE.md and docs/DEVELOPMENT.md apply in full. In
particular, no unit tests except where AGENTS.md allows them, the migration policy,
the layout contract tests, and the checks list. In addition:

### 1.1 Build new; do not reuse the old depth path

The current depth path (everything in §8) is being replaced because its structure is
wrong, not because its tuning is off. It resamples and filters depth about eight times
at five resolutions, gives each pixel one blended depth, and has no consistent camera
(§2). So:

- Do not call, wrap, copy or adapt functions, structs or constants from the files and
  functions listed in §8. Do not keep any of them alive, except the one labelled
  bridge in P1 (§7, T1).
- Do not port the old constants. They were tuned to hide the old structure's faults:
  sky fade widths, reach cutoffs, closeness ranges, scene-map cell sizes, march step
  counts, ground thresholds.
- New code lives in new modules (§4.7). Rewriting an old file's contents in place is
  fine where the registry requires the name (`apply_relight` in
  `mask_effects/relight.wgsl`), but none of the old body survives.
- **Allowed to carry over, rewritten into the new modules (not imported):**
  1. The look formulas listed per effect under "Keep" in §5. They define how a
     control maps to the image, and the user has approved them.
  2. Generic infrastructure that is not part of the depth path:
     - the effect registry (`mask_effects!`, `EffectSettings`, lanes, `EffectInstances`);
     - pass and bind-group machinery, `ShaderManager`, the layout contract tests;
     - the `FullImageMap` accumulate/resolve pattern for export tiles;
     - the AI model registry, sessions and jobs;
     - the sidecar asset table;
     - the migration pattern of `migrations/effect_sliders.rs`.
- If you believe you need something from the old path, stop and write the reason
  into §11 instead of using it.

### 1.2 Working conventions (the user's)

- Do not commit. Leave changes in the working tree.
- Do not launch the GUI app. Judge results with headless renders (§9.2). Ask the user
  to judge in the app on desktop and Android at the points marked "user review".
- Photos: use only `/run/media/duecki/Shutter_Chan/Demo/**`, the user's file
  `/run/media/duecki/Shutter_Chan/TestFiles/k you ve.CR3`, and photos the user
  names for this work. Never search other folders.
- Android test device: the Lenovo Tab M11 (adb serial `HA227JJC`, Mali-G52 MC2,
  4 GB) is the minimum device. CalibRaw on it may be replaced with debug builds;
  touch nothing else on the device. Ask the user to connect it if it is absent.
- Render-parity goldens (`data/render-parity/**/*.png`) are gitignored and exist only
  locally. `CALIBRAW_PARITY_BLESS=1` rewrites all of them. Copy `data/render-parity`
  somewhere outside the repo before every bless. Run the unblessed test first, so a
  change is checked against the old goldens.
- Before blaming effect code for an artifact, look at the depth, normal and sky maps
  themselves.
- When you check edges, overlay them across whole regions, never along a single row.

---

## 2. What is wrong today

The user reported four screenshots. They are described here because you cannot see
them.

| # | What the user saw | Effect | Cause in today's code |
|---|---|---|---|
| S1 | Indoor wet floor. Floor between and behind a person's boots, toward the back wall, has no reflection; reflections stop at a seam; near a leaning pole and a punching bag they look wrong | Wet Surfaces | Reflections mirror about a "contact row" found by walking up a 128² ground map. The walk stops at the boots and bench. The floor near the wall fails the "slopes toward the horizon" and "runs on toward the viewer" tests. The reflected colour comes from the environment grid, cells of 1/256 of the long edge (~23 px at 24 MP) |
| S2 | Face in profile against a black background. A stair-stepped skin-coloured band along the nose and lips, faint glow in the black area beside the face | Relight (rim) | Light follows the 1024×1024 square depth texture (~6×4 photo px per texel) and the guided filter on 256-long cells (3×3 window, then averaged over 3×3 more: 70–120 px). Where brightness "does not explain" depth, stored depth is mixed back in, which gives the steps. The wide windows carry the face's depth and light into the background (80–100 px measured on 2026-11-02) |
| S3 | Person in front of a wall. The cast shadow on the wall is a large black rounded blob with a smoky edge that does not follow the head and ears | Relight shadows | Marched through mip levels of the 1024² heightfield with a minimum footprint of 4 texels (~24 px). Every occluder is "solid for 15% of its distance", and the depth range is set by Relief, not measured |
| S4 | `k you ve.CR3`: stone cairn, sunset sky, defocused tree line. A pale line along the tree tops | Relight | Fixed ~72 px sky fade against a 70–190 px photo edge. A separate far-reach cutoff contour at 0.92–0.96. A sky mask that runs ~100 px below the horizon (measured 2026-11-04) |
| S5 | `Demo2.5/DSC09938 (1).ARW` (reported after P1): plush toy on a road, Relight behind it (light depth 100), Fog. A thinner-fog band and a lit fringe along the head and ears; with both effects, the head's shadow in the fog is drawn in steps | Fog, Relight, air shadows | Depth itself is clean (the map's edge lies within 10–20 px of the ears). The 1024² square texture (7×5 photo px per texel) and the brightness-only guided filter on 27 px cells (3×3-cell window) cannot separate a light-grey toy from a light-grey background, so the background beside the head gets an in-between depth. The air shadows are 16 slices of 128² over the frame: 55×37 px per texel at 7028 px; the shadowed band beside the head measures ~110 px. Identical with the pre-P1 depth of the same raw (`DSC09938.ARW`) |

Root causes shared by all of them:

1. **One depth per pixel.** A pixel that is half face and half background gets a depth
   in between, which no surface has. Every halo comes from this, and so does every
   patch that tries to hide one: the sky fade, the `solid` cap, the reach cutoff,
   closeness weights, and mixing stored depth back in.
2. **No consistent camera.**
   - Distance is guessed: `INVERSE_DEPTH_SHIFT = 0.115`, fitted on one photo.
   - The field of view is a constant: `SCENE_FOCAL_TAN = 0.4`.
   - The Relief slider sets the depth range.
   - MoGe's normals belong to the real camera, but are converted into depth gradients
     with a fitted scale.
3. **Too many resamples.** The model output at 1540 px goes through:
   1. a 1% clip (the farthest 1% becomes the sky value 255);
   2. enlarging to the photo size;
   3. sky forced to 255;
   4. the CPU guided filter `align_edges`;
   5. 8-bit quantization;
   6. a 1024² square texture;
   7. CPU dequantize and gradients;
   8. the GPU guided filter on cells;
   9. per-effect tents and weights.

---

## 3. Decisions already taken

If the user disagrees with any of these, they change them here before P1 starts.

1. **Old edits.** Where converting data and parameters can make them look nearly the
   same, a migration does so (§7). Where it cannot, they are converted and look
   different, listed as accepted regressions in §7.3. No old rendering code is kept
   for them.
2. **Stored depth.** Depth is stored as 16-bit disparity at the depth model's size,
   beside a soft sky probability, the normals and a camera block (§4.1). Sidecars
   holding it are written with schema 5.
3. **MoGe points.** Distance and focal length are measured from MoGe's 3D points. The
   shipped MoGe-2 ONNX files (ViT-S and ViT-L, unchanged from Ruicheng's exports)
   already output `points` in a pinhole camera with square pixels, besides `normal`,
   `mask` and `scale`: no re-export is needed (checked 2026-11-06, §12).
4. **Relief.** Relief becomes a scale of the scene's depth range, where 100% is the
   range the depth gives. Old values are migrated (M2).
5. **Depth Range masks.** They keep selecting on the depth exactly as today, with
   identical values on migrated files. Edge-accurate depth masks are optional P9, only
   if the user asks.
6. **Release assumption.** The rework ships in v1.5.0 (the workspace is at 1.4.0, and
   `v1.4.0` is tagged). User-file migrations are therefore marked
   `migration: remove in v2.0.0`. App-data cleanups would be marked
   `migration: remove in v1.7.0`. If the rework ships in a later release, recompute
   both markers with AGENTS.md's rule before finishing P7.
7. **No release mid-rework.** Do not cut a release between P1 and P7. If one must
   happen, the P1 bridge renders today's look and may ship, but P7 still deletes it.

Facts that shaped these decisions:
- v1.4.0 (2026-10-30) did not ship Distill depth, MoGe normals, effect instances,
  Rain or Wet Surfaces. Sidecars from v1.4 and older hold an 8-bit DA2- or DA3-era
  depth map without normals.
- Sidecars written on development builds since then also hold MoGe normals.
- Edit history is not persisted. Presets never carry depth.

---

## 4. Target design

### 4.1 Depth data and the scene camera

A depth result in `calibraw-core` is one value. The suggested name is `SceneDepth`; it
replaces `MaskStack::scene_depth: Option<MaskImage>` and `scene_normals`. It holds:

- **`disparity`**: 16-bit, at the depth model's output size (1540×1036 for 3:2). It
  is the model's raw output normalized to [0, 1] by its own minimum and maximum, with
  no clipping, high = near. Call a value `i`. A result migrated by M1 keeps its stored
  map's size (up to 4096 px), so Depth Range stays pixel-identical; the bake reads any
  size.
- **`sky`**: 8-bit sky probability at MoGe's size, soft (M1: at the map's size).
- **`normals`**: `SceneNormals`, as today (two 8-bit assets). It is always present
  (§7, M1 derives normals for old files).
- **`camera`**, a few scalars:
  - `focal_tan`: tan of half the field of view across the shorter edge.
  - `fit = (s, t)`: relative distance `D(i) = 1 / (s·i + t)`.
  - `anchors = (i_near, i_far)`: the disparity of the nearest and the farthest
    non-sky surface.
  - `frame_relief R`: the depth range of the shading frame, in units of the nearest
    distance.

The coordinates every effect uses are derived from these, in `scene/camera.wgsl`:

```text
D_near = D(i_near),  D_far = D(i_far),  Q = D_far / D_near
d(i)   = clamp((D(i) − D_near) / (D_far − D_near), 0, 1)    linear depth: media integrals
c(i)   = ln(D(i) / D_near) / ln(Q)                         control depth: sliders
d_of_c(c) = (Q^c − 1) / (Q − 1)                            control depth → linear depth
z      = 1 + E · R · d                                     shading frame, nearest surface at z = 1
         E = Relight's relief scale (1 everywhere else)
lateral position = 2 · focal_tan · (image point − centre) · z   (image points in shorter-edge units)
```

- Sky is not a surface. It has no light and no shadow. For media (Fog, Rain mist) it
  counts as `d = 1`.
- Light depth below 0 (toward the camera) stays `z = 1 + 0.9·depth`.

New depth results set:
- `R = Q − 1`, so `z = D / D_near`. This is the true camera frame, consistent with
  MoGe's normals.
- `focal_tan` from the photo's 35 mm-equivalent focal length:
  `tan = 21.633 · short / sqrt(w² + h²) / f35`. Perspective correction already reads
  this focal length. When it is missing, `focal_tan` comes from MoGe's points by
  least squares of `(u − c)·z = f·x`.

Why sliders use `c`, not `d`:
- With measured distances, an outdoor photo's range is 1000× or more. A linear slider
  would put the whole useful range of Relight's Light depth and Fog's Start into its
  first percent.
- `c` is even in the log of distance and is exactly reversible.
- For old data it reproduces the old positions exactly (M2, M3).

### 4.2 Two surfaces per pixel

A pixel is `coverage × near surface + (1 − coverage) × far surface`:
- Away from depth edges, coverage is 1 and only the near surface exists. That is
  ≥ 95% of pixels.
- Along an edge, the near and far surfaces each have their own depth, normal and
  colour.
- The sky is a far surface.

Effects shade each surface separately and blend by coverage. They never see a depth
between two surfaces.

The two colours are split from the pixel's own colour so that they add back to it
exactly: `F' = k·F`, `B' = k·B` per channel with `k = rgb / (α·F + (1 − α)·B)`. An
effect therefore never loses the photo's detail. (Changed 2026-11-06 from adding the
residual `r = rgb − (α·F + (1 − α)·B)` to both: with a bright sky behind a dark tree
line, a coverage a little under 1 turned `F + r` negative, and those pixels got no
light. The residual is still added where the blend is nearly black.)

### 4.3 Scene bake: once per depth result and unedited scene

The working size is the depth's aspect ratio, 1540 px on the long edge (or the depth's
own size if smaller; 1152 on memory-constrained devices if §10 requires it). It runs
in the full-image frame, so every crop and export tile reads the same result.

**Inputs:**
- the depth result;
- the working image: the unedited scene, scene-linear Rec.2020, unexposed, at the
  working size.
  - The full-frame preview pipeline builds the working image from its scene source.
  - Export tiles box-downsample their cores into it during the tone pre-pass, like
    the scene map's accumulation today.
  - Zoomed crops share it.

It is rebuilt on the same triggers as today's scene map, and on a new depth result.

**Passes:**

| Pass | Output (suggested format) | Notes |
|---|---|---|
| B1 Distance | per pixel `D`, `d`, `z` | from disparity via the camera block; sky probability upsampled bilinearly |
| B2 Edge band | `z_near`, `z_far`, band flag, sky flag (rgba16float) | band where `z_far / z_near − 1 > τ` within the model's ramp radius (6–10 working px), near = min, far = max (separable min/max); sky band where sky probability is between 0.02 and 0.98 nearby |
| B3 Coverage | guided-filter coefficients `a.rgb, b` (rgba16float) | colour guided feathering (He, Sun, Tang 2010, §4.3) of the near mask (`z < sqrt(z_near·z_far)`; for sky, `1 − sky`) against the working image. Radius follows the lens blur at the near depth (B7): compute 2–3 fixed radii (e.g. 2, 5, 12 px) and blend per pixel, so a defocused edge feathers over its real blur width and a sharp one stays sharp. Where the photo shows no edge, regularization gives a smooth ramp: this is the "100% feather where the edge cannot be seen" the user asked for |
| B4 Layer colours | `F`, `B` (2 × rg11b10ufloat) | push-pull fill inside the band from pixels with coverage ≈ 1 (F) and ≈ 0 (B) |
| B5 Layer normals | near and far normals, octahedral (rgba8snorm) | MoGe normals upsampled, then push-pull per side inside the band, as B4 |
| B6 Hi-Z | min `z` of the visible surface, in the working image's alpha, min-reduced down its mip chain | visible = near where coverage ≥ 0.5, else far; for Wet Surfaces' trace, which takes thickness from the bake's near/far `z` (§4.6 texture count) |
| B7 Lens blur | `c`, `z_f`, `σ0` of `σ(z) = c·|1/z − 1/z_f| + σ0` (uniform written by a one-workgroup pass, no readback) | from widths of strong edges measured on the working image (a 35 px blur at 6000 px is ~9 px here), robust fit; replaces the per-cell table |
| B8 Ground (CPU) | plane normal, offset, camera height, horizon, has-ground (uniform) | RANSAC on a ~192×128 point sample whose normals face up, computed on the CPU when depth is uploaded (< 2 ms) |

**Per pixel at full resolution:**
- Outside the band, coverage is 1.
- Inside it, coverage is `clamp(dot(a, rgb_scene(pos)) + b, 0, 1)`, with `a` and `b`
  bilinear at `pos` and `rgb_scene` the pixel's unedited scene colour. This is the
  "fast guided filter" evaluation: edges land on the photo's edges with sub-pixel
  precision, and the effect reads only its own pixel.

**Sharing:** one baked set per (device, depth result, scene source). All preview
pipelines of the open photo bind the same textures, held by `Arc`. It has a GPU budget
reservation of its own, like the LUT texture, and is not copied per pipeline. Export
bakes its own set from its tile accumulation.

### 4.4 Light bake: when a light moves or resizes, or the geometry changes

**L1 Shadow cube per light that casts shadows or lights a receiver.**
- Draw the depth grid as a mesh with a render pipeline. It is the first render
  pipeline in `calibraw-gpu`, so add layout contract coverage.
- The vertex shader builds the grid from `vertex_index`, with no vertex buffer.
- The mesh is the visible surface. A triangle across a band edge becomes a short
  **skirt**: its far-side vertices are the near-side positions pushed back along the
  view ray by `min(z_far − z_near, T·z_near)`, with `T` tuned in P3. This keeps light
  from leaking through the gap behind silhouettes without a rubber sheet down to the
  background.
- Use a `Depth32Float` 2D array, 6 layers per light. Faces are 512² on desktop and
  256² on constrained devices. Skip faces that see no geometry.
- While a light is being dragged, draw the mesh at half the working size. Redraw at
  full size when the drag ends.

**L2 Visibility per surface.**
- Compute it at the working size, per light, separately for the near and the far
  surface.
- Use PCSS: the light radius comes from Size, and penumbrae widen with distance by
  themselves.
- Use normal-offset and slope-scaled bias from the surface's normal.
- Store it as an `rgba8unorm` 2D array: four lights per layer, and a near and a far
  layer per group of four. All 8 lights get shadows, and the per-pixel trace for
  lights 5–8 disappears.

**L3 Air light: a march per pixel, in two quality tiers.** (Decided 2026-11-06,
§12 item 6. A froxel volume at working/8 was planned first; at ~37 photo px per froxel
it would draw S5's shafts in steps again.)
- A compute pass marches each pixel's view ray from the camera to its surface through
  the light's shadow cube (L1). It runs at a fraction of the pipeline's own output
  size, in the full-image frame, so a zoomed crop gets sharper shafts and the fitted
  preview stays cheap.
- Steps are even in `c`, plus steps before the nearest surface, with a per-pixel
  jitter hashed from the full-image pixel position (deterministic; no time or frame
  dependence).
- Per light it stores the **shadowed share** of the in-scattered light along the ray:
  `∑ visibility · w / ∑ w`, with `w` the light's falloff along the ray. It does not
  depend on density, phase, colour or any media slider, so moving a media slider
  rebuilds nothing; the media multiply their analytic, unshadowed in-scattering by it.
  Band pixels get the share at the near surface and at the far surface (one march,
  read at both depths).
- An edge-aware upsample (joint bilateral against full-resolution `z` and coverage)
  brings it to output size, so shafts stop on the photo's silhouettes. Its small
  support also smooths the jitter. Export tiles march their core plus that support,
  so results do not depend on the tile size.
- **Tiers:**

  | Tier | When | March size | Steps | Cube taps |
  |---|---|---|---|---|
  | Draft | while a control, a light handle or the view is dragged (the UI's drag phase, `interactive_preview.rs`); always in the reduced drag preview | ¼ of the output edge | 16 | 1 |
  | Final | when the preview is idle, and every export | ½ of the output edge | 48 | 4 (PCF) |

  The preview renders Draft while dragging and Final once the drag ends (the same
  "sharp after release" refine the reduced preview does today). The L1 cube follows
  the same tiers: half mesh and 256² faces in Draft, full in Final (desktop).
- Drops in the air need visibility at a point, not along a ray: they read the cube
  directly (`scene_point_visibility`).

**Keys:**
- L1: the geometry, each light's position, the relief scale and the tier.
- L2: also Size.
- L3: as L1, plus the pipeline's output region and size.
- Changing Amount, Colour, Ambient or any media slider rebuilds nothing.

### 4.5 Per-pixel API (`shaders/scene/geometry.wgsl`; signatures are suggestions)

```wgsl
struct SceneSurface { position: vec3<f32>, normal: vec3<f32>, color: vec3<f32>,
                      d: f32, c: f32, is_sky: bool, layer: u32 }
struct ScenePixel   { coverage: f32, near: SceneSurface, far: SceneSurface, in_band: bool }

fn scene_pixel(pos: vec2<i32>, rgb: vec3<f32>, relief_scale: f32) -> ScenePixel
fn scene_visibility(surface: SceneSurface, uv: vec2<f32>, light: SceneLight) -> f32
fn scene_air_light(pos: vec2<i32>, surface: SceneSurface, light: SceneLight) -> f32  // L3 shadowed share
fn scene_point_visibility(position: vec3<f32>, light: SceneLight) -> f32         // drops
fn scene_focus_blur(z: f32) -> f32          // Gaussian deviation, shorter edges
fn scene_ground() -> SceneGround            // plane, horizon, camera height
fn scene_trace(origin: SceneSurface, dir: vec3<f32>, roughness: f32) -> SceneTraceHit
fn scene_image(uv: vec2<f32>, footprint: f32) -> vec3<f32>   // working image, mip by footprint
```

Every depth effect is written as `shade(surface, ...) -> vec3` plus the shared blend
`scene_blend(pixel, near_rgb, far_rgb)`. Only band pixels shade twice.

### 4.6 Hard GPU constraints

- **Texture limit.** The creative pass is at the limit of 16 sampled textures per
  stage. The new inputs must fit by packing, using arrays where possible. The old
  inputs they replace are removed in the same phase as their last reader (P7 at the
  latest), so count both while they coexist. If they don't fit, stop and report.
  - Counted 2026-11-06: `bgl_adjust_creative` binds exactly 16 sampled textures, and
    the Wet Surfaces and weather-air passes use the same layout. Eight of them belong
    to the old depth path: 35 depth/surface, 46 shadow map, 69/70 environment grid and
    its blur, 71 guided-filter coefficients, 73 ground, 75 focus, 77 depth range.
  - **Decided (§12 item 3, option a):** the bake is one `rgba16float` 2D array
    (geometry, coverage, layer normals, `F`, `B`). Every effect reads 35 and
    71/73/75/77 through the shared `scene_depth_at` until P7, so the count must hold
    in every phase:

    | Phase | Change | Sampled textures |
    |---|---|---|
    | today | | 16 |
    | P4 | under the bridge tag, merge 71/73/75/77 into one array (padded to the largest): −3; add the bake array and L2's visibility array: +2 | 15 |
    | P5 | add the L3 air-light texture, the L1 cube array (drops) and the working image: +3. Rain stops reading the environment grid; under the bridge tag, its last reader (Wet Surfaces, until P6) samples the working image's mips instead, and 69/70 are deleted: −2 | 16 |
    | P6 | Hi-Z (B6) lives in the working image's alpha channel (its own min-reduced mip chain; thickness comes from the bake's near/far `z`), so Wet Surfaces adds nothing | 16 |
    | P7 | delete 35, 46 and the merged array | ≤ 13 |

    Re-count at the start of P4, P5 and P6 and record the count in §11. The P5 bridge
    change alters Wet Surfaces' reflections slightly until P6; re-bless and say so.
- **No texture as a function argument.** It crashes the Mali-G710 shader compiler
  (`no_handle_arguments` test). Read textures through the global bindings inside
  helpers.
- **Mip sizes.** Derive mip sizes from level 0; lavapipe reports wrong sizes for
  higher levels.
- **Filtering.** `r32float` is not filterable without a feature. Use 16-bit float
  formats or `textureLoad` with manual filtering.
- **Full-image frame.** Everything is in the full-image frame and independent of tile
  size. Relight, Fog and Smoke read only their own pixel, so their `EffectReach`
  becomes `Pixel` (today it is `SCENE_DEPTH_GUIDE_SUPPORT`). Wet Surfaces keeps a
  small reach for its grain.
- **Determinism.** Results must not depend on atomics order or timing; parity renders
  must be repeatable.
- **Documentation.** Document colour spaces, coordinate systems (the formulas of
  §4.1), boundary handling and precision in every new WGSL file (AGENTS.md).

### 4.7 New module layout (suggested)

- `calibraw-core`:
  - `pipeline/scene_depth.rs`: `SceneDepth` and the camera block.
  - Sidecar assets: 16-bit gray PNG and the sky asset, in `sidecar/mask_assets.rs`.
- `calibraw-ai`: `models/depth.rs` and `models/scene_geometry.rs`, post-processing
  rewritten (P1); `packaging/models/moge_export.py`.
- `calibraw-gpu/src/pipeline/gpu/scene/`:
  - `mod.rs`: the shared set and its keys;
  - `camera.rs`;
  - `bake.rs`;
  - `ground.rs`;
  - `lights.rs`: cubes, visibility, air light and its quality tier.
- `calibraw-gpu/src/shaders/scene/`:
  - `camera.wgsl`, `geometry.wgsl`;
  - `bake_*.wgsl`;
  - `shadow_raster.wgsl` (vertex and fragment);
  - `visibility.wgsl`, `air_light.wgsl`, `trace.wgsl`, `focus.wgsl`.
- Registry: replace `SceneMapUse` with what an effect needs:
  - geometry;
  - working image (also without depth, for Rain and Wet Surfaces' environment);
  - light bake;
  - air light.

---

## 5. Effects

### Relight
- **New behaviour:**
  - Shade each surface with its own normal. Size wraps the Lambert term, and Size
    also softens the normal over a disc whose taps are rejected by distance from the
    surface's plane.
  - Use inverse-square falloff in the frame, visibility from L2, and reflectance
    from that surface's colour.
  - The sky gets nothing because it is not a surface, not because of a cutoff.
  - Light placement: Light X/Y as today; Light depth in `c` (§4.1).
  - Relief becomes `relief_scale` E (10–300%, default 100%).
- **Keep:**
  - the reflectance estimate (`solid · pow(clamp(luma/ambient, 0.02, 64), −0.3)`);
  - the wrapped Lambert `wrap = 0.5·size`;
  - falloff `1/(1 + r²/reach²)` with `reach = clamp(Reach/100, 0.1, 4)·2·focal_tan`;
  - `SCENE_LIGHT_GAIN` 2.5;
  - Ambient multiplying the input;
  - lights adding up on one reflectance estimate;
  - the Shadows strength mix and the shadows on/off toggle.
- **Drop:**
  - `relight_smoothed_gradient`, `relight_reach` and the `RELIGHT_REACH_*` cutoff;
  - `relight_visibility`, `relight_blocked`, `relight_step_unoccluded`;
  - the 16-tap surface tents, the four-channel shadow limit and the per-pixel trace
    fallback.
- **Fixes:** S2, S3, S4.

### Fog, Smoke, Rain mist (shared medium)
- **New behaviour:**
  - Optical length to each surface's own `d`; the sky counts as `d = 1`.
  - Band pixels fog each surface and blend, so silhouettes and tree lines get no rim.
  - In-scattered scene light is the analytic, unshadowed integral (falloff ×
    Henyey–Greenstein phase) times L3's shadowed share for the pixel's surface; there
    are no wisps or voxel blocks, and shafts follow silhouettes at the output's
    resolution.
  - Fog Start is in `c`.
- **Keep:**
  - Fog's whole look model: onset integral, bank-density noise, Beer–Lambert
    `6·density²·amount·length·|ray|` with today's `image_point·1.25` ray obliqueness,
    airlight `colour·ambient·1.15`, the Light glow gain and image lights.
  - Smoke's image-space sheets.
  - The phase g values (Fog 0.6, Smoke 0.4, drops 0.72).
  - The 12-cell integration.
- **Drop:** the 16 × 128² air shadow slices and their march.

### Rain
- **New behaviour:**
  - Drops are hidden per surface: a drop between the near and the far surface shows
    through the uncovered part.
  - Defocus comes from B7.
  - Slabs are spaced in `c`.
  - Splashes land on the B8 plane.
  - Drop light uses `scene_point_visibility`, mist uses L3, and the refracted surroundings come from `scene_image` with a
    wide footprint.
- **Keep:** streak length from the shutter (square root), drop and splash shapes,
  opacity, gusts, perspective, `RAIN_MOST_CELLS`, and the constants the 2026-11-02
  rewrite chose.

### Wet Surfaces
- **New behaviour:**
  - **Where it's wet:** ground is where the surface normal lies within about 20° of
    the plane normal and the surface lies near the plane. In band pixels only the
    surface that is ground gets wet.
  - **Reflections:** `scene_trace` reflects about the plane normal tilted by ripples,
    marches in screen space through Hi-Z against near and far `z`, and reads
    `scene_image` at the hit with a footprint from roughness × travel. A miss fades to
    the wide-footprint image along the exit direction.
  - **Puddles:** the field and glints use plane coordinates from B8 and `focal_tan`.
  - **Distance fade:** the far fade comes from the ground footprint, not from a depth
    threshold.
- **Keep:**
  - the Lekner–Dorf darkening;
  - the puddle field: fbm octaves, threshold from Puddles, size frequencies;
  - grain-broken edges (`wet_detail` idea), lens-matched edge ramps, soak;
  - ripples, Fresnel, the sheen/gloss/roughness mappings, GGX glints.
- **Drop:**
  - the contact distance and the mirrored reflections;
  - the ground score, support and beside tests;
  - terrain from rings; `WET_FAR_*`;
  - the environment grid.
- **Fixes:** S1.

### Scene emission (Glow, Neon), Light Rays as a light
- Port them to `scene_pixel`, visibility and the frame. Keep their formulas and grids.
  Emitters still cast no shadows, which is not in scope.

### Depth Range masks
- Select on `b' = clamp(1 − (i − i_far)/(i_near − i_far), 0, 1)` in
  `calibraw-core/.../masks/raster.rs`. For migrated files `b'` equals the old stored
  value exactly, so masks do not change. P9 (optional) would add coverage-accurate
  edges.

---

## 6. Phases

Each phase ends with:
- the full checks in AGENTS.md (`fmt`, `clippy`, `test`, `arch-check`, `migrations`,
  `deny`);
- headless renders of the reference set (§9.1) at 100% and 200%;
- stage timings on desktop and on the Tab M11 when the phase touches the GPU (§10);
- a filled row in §11.

### P0 Baseline (no code changes)
1. Copy `data/render-parity` outside the repo.
2. Ask the user for the photo files (and sidecars) behind S1, S2 and S3.
3. Render the reference set (§9.1) and S1–S3 with today's code. Use 16-bit PNG, keep
   the renders outside the repo, and render each effect toggled on and off.
4. Record stage timings on desktop and on the Tab M11 for:
   - Relight (1 light with shadows);
   - Relight (4 lights with shadows);
   - Fog with Light glow;
   - Rain with mist;
   - Wet Surfaces with puddles and reflections.

   Use DSC08405 (1) and S3's photo. Include memory use of the scene inputs.

### P1 Depth data and storage
1. **`calibraw-core`:**
   - Add `SceneDepth` with its camera block (§4.1), replacing `scene_depth` and
     `scene_normals`.
   - Make Depth Range components reference it.
   - Add the 16-bit and sky assets, the `scene_camera` JSON object and schema 5, with
     validation and `size_limits.rs`.
   - Write M1 and M4 (§7) with format fixtures.
   - Point Depth Range rasterization at `b'`.
   - Update presets' depth clearing, `content_dependencies.rs`, and the AI job state
     that stores the result.
2. **`calibraw-ai`:**
   - Rewrite depth post-processing:
     - Distill's raw output is min/max-normalized, with no clip and no inversion,
       stored at the model's size in 16 bits.
     - MoGe gives normals, sky probability and points.
     - Fit `s, t` by robust IRLS (Huber) in inverse depth on non-sky pixels, with
       MoGe resized to the depth size.
     - Anchors are the 0.5th and 99.5th percentiles of non-sky disparity.
     - `R = Q − 1`.
     - `focal_tan` comes from the 35 mm focal length passed into the job, else from
       MoGe.
   - Delete `depth_refine.rs`, `make_sky_farthest`, the 1% clip and `quantize_depth`.
   - Read MoGe's `points` output from the shipped artifact (no re-export, §3 item 3).
   - Re-bless the AI parity depth golden and review it.
3. **`calibraw-gpu`:** add the temporary bridge T1 (§7), so every current effect
   renders from the new data as before.
4. **Acceptance:**
   - Format fixtures pass.
   - Render-parity passes unblessed: migrated sidecars plus the bridge reproduce the
     old stored values exactly.
   - Depth Range masks on a migrated Demo sidecar are pixel-identical.
   - A fresh depth of DSC08405 (1) shows continuous wires and no clipped far scenery
     marked as sky (inspect the maps).
   - On DSC05155 top, DSC08714 houses and DSC05314 building edge, compare `D` against
     MoGe's distance: report blotches. A previous merge attempt produced them there;
     if they appear, stop and report.

### P2 Scene bake and geometry API
1. Add the module and WGSL directory (§4.7), the shared set with its budget
   reservation, the working-image accumulation (preview, export tone pre-pass, crops),
   passes B1–B8, and the registry needs.
2. No effect uses the bake yet. To inspect its textures, use a temporary local debug
   path and delete it before the phase ends. Never leave a debug or test-only API
   (AGENTS.md).
3. **Acceptance** on DSC05314, DSC08405 (1), `k you ve.CR3` and S2's photo:
   - Sharp edges: the 50% coverage contour lies within 1.5 full-resolution px of the
     photo's strongest edge.
   - Defocused edges: the coverage ramp's 10–90% width is between 0.8 and 1.25 times
     the photo's own.
   - No blocks or steps anywhere along silhouettes (overlay across whole regions).
   - New depth shows no 8-bit terraces in `z`.
   - The thin-lens fit reads 30–40 px at the far tree line of `k you ve.CR3` at full
     size. Today's table reads 16–20 against an estimated 35.
   - Bake time within §10 on desktop and the Tab M11.

### P3 Light bake
1. Build the shadow cube render pipeline with skirts, face culling and decimation
   while dragging. Add L2 and L3 (the per-pixel air-light march, its edge-aware
   upsample and the Draft/Final tiers of §4.4, driven by the UI's drag phase), their
   keys and invalidation, and layout contract entries.
2. **Acceptance:**
   - A synthetic case (box and sphere in front of a wall) casts shadows matching the
     silhouettes as seen from the light.
   - On S3's photo the wall shadow follows the head and hair outline.
   - No light leaks at silhouettes with a grazing light.
   - No acne on curved surfaces (DSC05314 toy).
   - Rebuild while dragging within §10 on the Tab M11.
   - L3 within §10 in both tiers on desktop and the Tab M11; a drag shows Draft and
     the preview refines to Final after release without a visible jump in brightness
     (both tiers estimate the same share).
   - L3 does not depend on the tile size (render at default and 64-px tiles).
   - If a missing cache invalidation causes a bug, a Rust test is allowed
     (AGENTS.md).

### P4 Relight
1. Rewrite Relight (§5) and its scene light, and write M2 (§7).
2. New Relief control spec. Update the locale catalogs for changed labels. Follow the
   UI rules: no tooltip on sliders; help goes on static labels only.
3. Add parity cases and bless them after a backup, reviewing each image:
   - `relight-silhouette-sky`;
   - `relight-defocused-horizon`;
   - `relight-cast-shadow`;
   - `relight-rim-behind`;
   - `relight-eight-lights-shadows`.
4. **Acceptance:**
   - S2: the rim ends on the photo's silhouette. Added light in a band 8–24 px
     outside the silhouette is < 5% of the light just inside, and there are no steps.
   - S3: the shadow shape matches the subject's outline as seen from the light; its
     edge widens with distance and Size; no smoky halo.
   - S4: no pale line along the tree tops; the added light's edge is no steeper than
     the photo's own horizon (today 0.93 levels/px against the photo's ~1.9 was still
     visible, so compare by eye at 100% too).
   - Regression set:
     - DSC05155: no contours in the sky.
     - DSC05314: no blotches on fur or the pole.
     - DSC08714: flat objects lit correctly.
   - Then: user review in the app on desktop and Android.

### P5 Media and Rain
1. Rewrite the shared medium, Fog (and M3), Smoke's lighting, Rain mist, drops and
   splashes (§5).
2. Add parity cases:
   - `fog-silhouette-sky`;
   - `fog-glow-light-behind-subject`;
   - `rain-occlusion-layers`.
3. **Acceptance:**
   - No fog rim on silhouettes or tree lines (DSC08062, DSC08405 (1) train edge).
   - Lit mist without wisps or voxel blocks: DSC05168 with Rain mist 90, Light glow
     100, Relight 100 at (58.5, 50.4), light depth 85. Note that light depth is
     migrated by M2.
   - Pillar light shafts on DSC05155 come only from real occluders.
   - S5 (`DSC09938 (1).ARW`, its sidecar's Relight with Fog enabled): no thinner-fog
     band or lit fringe at the head and ears, and the head's shadow in the fog follows
     its outline at 100% with no steps.
   - Then: user review.

### P6 Wet Surfaces
1. Rewrite Wet Surfaces (§5).
2. Add parity cases: `wet-object-on-floor`, `wet-floor-to-wall`.
3. **Acceptance:**
   - S1: the floor between and behind the boots reflects what stands behind it (boots,
     bench, wall). No seam ends the reflections. Reflections are sharp where the floor
     is smooth.
   - DSC09938 floor: no bright sky tendrils.
   - DSC05155 platform, DSC04230 (Demo2) kerb and DSC07940 (Demo2.5) grass: no
     pasted-in puddles; puddle edges as soft as the photo there.
   - Then: user review.

### P7 Port and delete
1. Port scene emission and Light Rays' scene light.
2. Delete everything in §8 and the bridge T1. Remove `SceneMapUse` leftovers and the
   environment grid.
3. Update docs/ARCHITECTURE.md:
   - the GPU resources section;
   - the `SHARED_SURFACE` row (replaced by the shared scene set);
   - the caches table.
4. Update the "Adding an effect" section of docs/DEVELOPMENT.md.
5. Re-check the migration markers against the actual release (§3, item 6).
6. **Acceptance:**
   - Every grep in §8 returns nothing.
   - The bridge tag returns nothing.
   - All parity cases pass, and the full checks pass.

### P8 Performance pass
Compare with P0 and §10 on desktop and the Tab M11. Tune working size, face size,
PCSS taps, and L3's march size and steps per tier. Record the results in §11.

### P9 Optional, only if the user asks: edge-accurate Depth Range masks
Rasterize Depth Range masks per surface with the coverage. This changes mask output,
so it would need its own decision.

---

## 7. Migrations, temporary code and accepted regressions

Every item here is labelled. Migration files live in `crates/calibraw-core/src/migrations/`,
carry the marker in their module comment, run when a file is decoded, set
`LoadedSidecar::migrated` (so the file is saved again in the new layout), are listed
in `migrations/mod.rs`, and get fixtures in `data/formats/`. Deleting a migration
deletes its tests and old-layout fixtures and is noted in the release notes
(AGENTS.md). Legacy constants appear only inside these files:

```text
LEGACY_SHIFT        = 0.115                          old distance: D = 1 / (i + 0.115)
LEGACY_FOCAL_TAN    = 0.4
LEGACY_FRAME_RELIEF = 0.25 + 5.75 · 0.5^1.5 = 2.28293  old default Relief, used by every effect
LEGACY_RANGE_RATIO  = 1.115 / 0.115 = 9.69565        Q of the old distance between stored 0 and 255
```

### 7.1 Migrations (shipped; removed in the stated version)

| Label | What it converts | File | Remove in | Result for old edits |
|---|---|---|---|---|
| **MIGRATION M1: scene depth layout** | Sidecars of schemas 1–4 with an 8-bit stored map `b` (near 0, sky and the farthest 1% = 255), with or without normals, including Depth Range components' references. Steps: (1) disparity `i = 1 − b/255` at the map's own size (not resampled: the map is up to 4096 px, and Depth Range must stay identical); (2) dequantize: edge-aware smoothing held within ±0.5/255 of each sample, no smoothing across steps > 2 levels or the sky; (3) sky probability 1 where `b = 255`, at the map's size; (4) camera block `focal_tan 0.4, fit (1, 0.115), anchors (1, 0), frame_relief 2.28293`; (5) if normals are missing, derive them from the dequantized depth in that frame (central differences, not across steps > 2 levels) and store them. Write fresh code; do not use `scene_surface.rs` or `scene_depth.rs`. Runs at document load, never on the UI thread | `scene_depth_layout.rs` | **v2.0.0** | `d`, `z` and the frame are exactly what the old effects computed; Depth Range masks are identical (`b' = b`). Differences come only from the new algorithms (§7.3) |
| **MIGRATION M2: Relight scene controls** | In sidecars and presets, every Relight, including `additional` lights. Steps: (1) `relief` (0–100, old key, read from the raw document as `LegacyEffectSliders` does) → `relief_scale` (%, new key) `= 100 · (0.25 + 5.75·(relief/100)^1.5) / 2.28293`, clamped to 10–300; (2) light `depth` (old key; linear depth) → `light_depth` (new key; control depth `c`): for `depth ≥ 0`, `100 · ln(1 + 8.69565·depth/100) / ln(9.69565)`; for `depth < 0`, unchanged | `scene_depth_controls.rs` | **v2.0.0** | On M1-migrated depth, light position, frame and Relief are exactly the old ones (old default Relief 50 → 100%). On regenerated depth the light keeps its `c` and Relief scale, so it sits at the same perceptual depth |
| **MIGRATION M3: Fog start** (same layout change as M2, same file) | Fog `start` (old key, 0–95, linear depth) → `start_depth` (new key, `c`) `= 100 · ln(1 + 8.69565·start/100) / ln(9.69565)`. The new default is the migrated old default, 8 → 23 | `scene_depth_controls.rs` | **v2.0.0** | Identical onset on M1-migrated depth |
| **MIGRATION M4: older readers** | Add to `older_readers.rs` (existing file, already marked): edits holding the new depth layout, Relight or Fog are written with schema 5, so v1.4 and older report them as newer instead of misreading `light_depth` / `start_depth` or the 16-bit assets. Presets: the same with `PRESET_SCHEMA_VERSION` | `older_readers.rs` | **v2.0.0** (existing marker) | Old app versions never overwrite a new file with a wrong one |
| ~~MIGRATION M5~~ | Dropped 2026-11-06: the current MoGe artifact already has the `points` output, so it stays | — | — | — |

Fixtures (`data/formats/`):
- a schema-4 sidecar with an 8-bit depth, normals, a Depth Range component, Relight
  (relief 20 and 90, two lights with depths −30 and 60) and Fog (start 8);
- the same without normals (a v1.4 file);
- a preset with Relight and Fog.

### 7.2 Temporary code (never a migration; must be gone at the end of P7)

| Label | What | Where | Remove in |
|---|---|---|---|
| **TEMPORARY T1: depth bridge** | Feeds the new `SceneDepth` into the old GPU upload as stored values `b' · 255` plus normals, so today's effects keep rendering during P1–P6. Mark every piece with the comment `depth-rework-bridge: delete in P7`. Do not use a `migration:` marker; it is not one | `calibraw-gpu` scene-depth upload | **P7** (`rg "depth-rework-bridge"` must return nothing) |
| **TEMPORARY T2: debug inspection** | Any local code that dumps bake or light textures | anywhere | the end of the phase that added it |

T1 was removed in P7 (2026-11-07): `rg "depth-rework-bridge"` returns nothing.

### 7.3 Accepted regressions (looks that change even after migration)

List these in the release notes of the release that ships the rework.

| Label | What changes | Why it is accepted |
|---|---|---|
| **REGRESSION R1** | Relight shadows have a new shape and edge, also on migrated edits | The old shapes were the bug (S3) |
| **REGRESSION R2** | Relight, Fog and Rain at silhouettes and sky edges: no halos or lines, and edge pixels shade differently | The fix for S2 and S4 |
| **REGRESSION R3** | Relight shading on development-build files with MoGe normals uses them directly, not through gradients; on v1.4 files M1's derived normals are close to the old gradients but not identical | Consistent camera |
| **REGRESSION R4** | On M1-migrated (old) depth, far scenery may get faint Relight light where the old 0.92–0.96 cutoff removed it, because that frame's range is compressed. Regenerating depth fixes it | The cutoff caused S4's contour |
| **REGRESSION R5** | Fog, Smoke, Rain light glow: air shadows come from shadow maps, so there are no wisps or blocks and shafts are sharper | Bug fix |
| **REGRESSION R6** | Wet Surfaces reflections are traced (content and sharpness differ), and puddle positions shift where the fitted horizon differs from the old estimate | The fix for S1 |
| **REGRESSION R7** | Rain slabs are spaced in `c`: drop size and density by distance differ (Rain is not in any release yet) | Outdoor ranges |
| **REGRESSION R8** | Regenerated depth changes every depth effect: measured distances, real field of view, a soft sky. Controls keep their meaning through `c` | The point of the rework |
| **REGRESSION R9** | Glow/Neon scene light and Light Rays' light: small differences from the new surfaces | Port |

---

## 8. Deletion list (all gone by the end of P7)

- **GPU crate (Rust):**
  - `crates/calibraw-gpu/src/pipeline/gpu/scene_surface.rs` (whole file).
  - In `crates/calibraw-gpu/src/pipeline/gpu/scene_depth.rs`: `SHARED_SURFACE`,
    `distance_from_stored`, `distance_per_stored`, `INVERSE_DEPTH_SHIFT`,
    `resampled_depth`, `SKY_STORED`, the encoded levels. The upload moves to `scene/`.
  - `crates/calibraw-gpu/src/pipeline/gpu/ground_plane.rs` (whole file).
- **Shaders:**
  - `shaders/mask_effects/scene_depth.wgsl` and `scene_shadows.wgsl` (whole files).
  - In `shaders/scene_analysis.wgsl`: the guided-filter coefficients, the depth range
    and depth edge, the sky window and fade inputs, the ground, contact, support,
    beside and terrain code, `build_scene_focus`, and the environment grid and its
    blur (replaced by the working image). Delete the file if nothing is left.
  - `scene_environment.wgsl`: `scene_environment_at` and
    `scene_environment_blurred_at`. Ambient level and image lights stay.
  - In `relight.wgsl`: everything named in §5 "Drop". In `wet_surfaces.wgsl`: the same.
  - In `rain.wgsl`, `medium.wgsl`, `scene_lights.wgsl` and `scene_emission.wgsl`:
    every read of the old textures and `scene_surface_taps_at`.
- **Constants and keys:** `SCENE_DEPTH_EDGE`, `SCENE_MAP_GRID_LONG` (if unused),
  `SCENE_FOCUS_BINS`, `SCENE_SKY_*`, `SCENE_SHADOW_*`, `RELIGHT_REACH_*`,
  `SCENE_FOCAL_TAN` (replaced by the camera block), `SCENE_DEFAULT_RELIEF_CONTROL`,
  `GpuParams::scene_shadow_map_key` and the old scene shadow map pass.
- **AI crate:** `crates/calibraw-ai/src/models/depth_refine.rs`; in `depth.rs`:
  `make_sky_farthest`, the 1% clip in `normalize_depth`, `quantize_depth`, the
  enlargement to photo size.
- **Export halos:** `SCENE_DEPTH_GUIDE_SUPPORT` halos for Relight, Fog and Smoke.
- **Check:** `rg "scene_surface_taps_at|SHARED_SURFACE|INVERSE_DEPTH_SHIFT|scene_depth_map_tex|scene_shadow_map\b|relight_blocked|depth-rework-bridge"`
  returns nothing.

---

## 9. Verification reference

### 9.1 Reference photos

All under `/run/media/duecki/Shutter_Chan/Demo/` (find their subfolders) unless noted.

| Photo | Check |
|---|---|
| S1, S2, S3 photos | the user names them in P0 |
| `/run/media/duecki/Shutter_Chan/TestFiles/k you ve.CR3` | S4: defocused tree line against a sunset sky |
| `Demo2.5/DSC08405 (1).ARW` | wires, fence, train edge; its sidecar has Glow, Fog (Light glow 50), Relight at (38.7, 46.5) depth 40 |
| DSC05155 | sky contours, platform puddles, pillar light shafts |
| DSC05314 | fur and the pole (blotches), building edge (fit blotches) |
| DSC08714 | flat objects (transformer), dense wires, houses (fit blotches) |
| DSC09938 | floor reflections (sky tendrils), background buildings |
| `Demo2.5/DSC09938 (1).ARW` | S5, the user's own test for silhouettes with a light behind the subject (Relight, Fog, light in the fog); same raw as DSC09938, with fresh depth |
| DSC08062 | fog edges |
| DSC05168 | lit mist (setup in P5) |
| `Demo2/DSC04230`, `Demo2.5/DSC07940` | kerb, grass puddles |
| DSC01000, DSC09953 | a thin strip at the frame border in stored depth: an upstream defect, do not chase it in effect code |

Demo sidecars mix DA3-era (linear in distance) and DA2-era maps, and nothing in a
sidecar says which. Judge looks only on freshly generated depth or on DA2-era maps.
Check the map's mean first: DA2-era means are about 170–185.

### 9.2 Headless renders

- Build a scratch crate outside the repo:
  - path dependencies on `calibraw-cli`, `calibraw-core` and `calibraw-gpu`;
  - copy the workspace `Cargo.lock`;
  - build with `--offline`.
- Load a RAW and its sidecar with `calibraw_core::sidecar::decode`, then export with
  `spawn_tiled_export` as `crates/calibraw-gpu/tests/render_parity.rs` does.
- Use 16-bit output for faint effects.
- Isolate an effect by rendering with its component's `enabled` set to false.
- Render at default tile size and at 64-px tiles: the results must match.

### 9.3 Commands

```sh
cargo test --locked -p calibraw-gpu --test render_parity
cargo test --locked -p calibraw-core --test formats
cargo test --locked -p calibraw-ai --test parity
cargo xtask migrations
# full list: AGENTS.md "Verification"
```

---

## 10. Performance and memory budgets

These are proposed targets; P0 records today's numbers and P8 tunes against them.

| Work | When it runs | Desktop | Tab M11 |
|---|---|---|---|
| Scene bake (B1–B8) | new depth or scene-map triggers | < 5 ms | < 120 ms, once per photo |
| Light bake (L1, L2) | light moved or resized | < 2 ms per light | < 25 ms per light while dragging (half mesh, 256² faces); full quality on release |
| Air light (L3), Draft | every frame while dragging, fitted preview | < 1 ms per light | < 8 ms per light |
| Air light (L3), Final | once after the drag ends; export | < 4 ms per light (fitted preview) | < 40 ms per light (fitted preview) |
| Per pixel | every edit | ≤ today | ≤ today; band pixels shade twice |

Reads per pixel:
- **Relight today:** 16 surface + 16 shadow + 12 normal + 8 reach + 13–29 sky taps.
- **Target:** about 4 geometry reads plus 1 visibility read per surface and four
  lights.

Memory at 1540×1036, one shared set, estimates:

| Item | Size |
|---|---|
| Geometry | ≈ 13 MB |
| Coverage coefficients | ≈ 13 MB |
| Layer normals | ≈ 6 MB |
| Layer colours | ≈ 13 MB |
| Hi-Z | ≈ 2 MB |
| Working image with mips | ≈ 17 MB |
| **Bake total** | **≈ 64 MB** |
| Per light: cube | 6 MB desktop, 1.6 MB constrained |
| Per four lights: visibility | 13 MB |
| Air light, per pipeline and four lights | `rgba16float` at the march size: ≈ 3.4 MB for a 1600 px preview in Final |

Today each preview pipeline holds about 25 MB of scene inputs on its own. If the Tab
M11 exceeds its budget, drop the working size to 1152 on constrained devices first.

---

## 11. Progress and measurements

Fill in one row per phase: what was done, timings, memory, open problems and anything
from the old path that seemed needed (§1.1).

| Phase | State | Notes |
|---|---|---|
| P0 | partly done | **Done:** `data/render-parity` and `data/ai-parity` copied outside the repo (session scratchpad, `backup/render-parity-94256c9c`, volatile: the goldens are reproducible by blessing `94256c9c` in a `git archive` copy); format fixtures and render parity green on `94256c9c`. **Open:** S1–S3 photo files (ask the user), the reference renders (can be made from `94256c9c` at any time, before P4), timings on the Tab M11 (not connected on 2026-11-06). |
| P1 | done, awaiting review | **Built:** `calibraw-core` `pipeline/scene_depth.rs` (`SceneDepth`, `DisparityMap`, `SceneCamera`; `selection_map` is what Depth Range selects on, in 255 levels as before); `MaskStack::scene_depth: Option<SceneDepth>` and `set_scene_depth` (Depth Range components hold the selection map, never stored); sidecar schema 5 (`scene_depth` object: 16-bit disparity, sky and normal assets, camera); M1 (`migrations/scene_depth_layout.rs`, v1.1 inline pixels included) and M4 (`older_readers`: schema 5 when depth is stored, schema 4 for Rain/Wet/instances). `calibraw-ai`: Distill min/max disparity at 1540×1036, MoGe points, Huber IRLS fit with a range fit as fallback, anchors at the 0.5/99.5 % of surface disparity, `R = Q − 1`, `focal_tan` from the 35 mm focal length (EXIF × lensfun crop factor) else MoGe's points; `depth_refine.rs`, `make_sky_farthest`, the clip and `quantize_depth` deleted. T1 bridge in `GpuParams::new` and `stage_params.rs` (3 tags). Fixtures `scene-depth-dev` (schema 4, normals, Depth Range, two Relights, Fog) and `scene-depth-v1.4` (schema 3, no normals). **Checks:** all of AGENTS.md pass. AI parity `depth-low`/`depth-medium` re-blessed: ViT-S camera tan 0.361, Q 18.4; ViT-L tan 0.460, Q 15.0 (DSC01000). **Acceptance:** M1 round trip exact (every migrated disparity rounds back to the stored map; fixture rows show it). Depth Range parity case (`mask-shaping`) unchanged. Render parity does **not** pass unblessed, by design of M1: the 8 depth cases without stored normals now get M1's derived normals, which today's shaders turn into gradients with a scale fitted to MoGe's camera (Wet Surfaces loses its ground there, Relight shades differently). With normals withheld from the bridge every case matched its old golden exactly, so the stored depth is reproduced exactly; the 8 goldens were re-blessed (`depth-effects`, `glow-scene-light`, `neon-scene-light`, `rain-wet-relight`, `relight`, `relight-lights`, `relight-sky`, `wet-surfaces`), all others byte-identical. Fresh depth (embedded previews, ViT-S, ~10 s CPU each): DSC08405 (1) wires continuous, far trees not marked sky. Fitted `D` vs MoGe's distance: median \|ln D/z\| 0.06–0.12, 95th percentile 0.33–0.91; Fog previews from `D` show **no blotches** on DSC05155 top, DSC08714 houses or DSC05314 building edge (smoother than MoGe's own distance), but the fit puts mid/far scenery farther than MoGe where Distill is not affine in inverse depth (DSC08714: train and houses fog more). A look question for P5, not a P1 blocker. Format test: mask assets now compared by decoded samples, since PNG bytes depend on the deflate backend of the build (`-p` vs workspace). |
| P2 | built, awaiting review | **Built** (module `calibraw-gpu/src/pipeline/gpu/scene/`: `mod.rs` resources, state and pipeline hooks, `bake.rs` programs and pass order, `camera.rs`, `ground.rs`; shaders `shaders/scene/camera.wgsl`, `bake_common.wgsl`, `bake_accumulate.wgsl`, `bake_depth.wgsl`, `bake_focus.wgsl`, `bake_coverage.wgsl`, `bake_layers.wgsl`, `bake_mips.wgsl`). Only the working-image accumulation is a pass of the processing pipeline; the bake's 14 passes have their own layouts, compile once per device and size their dispatches from the working size. Working size `min(1540, depth's, source's long edge)` at the image's aspect, 1152 below a 1 GiB GPU budget. Bake array `rgba16float`, 5 layers (geometry `(d, sky, d_near, d_far)`, coverage `(a.rgb, b)`, normals octahedral near/far, near colour + edge flag, far colour + edge flag); working image with mips (alpha from level 1: least visible `d`, B6); one scratch buffer reused by the accumulation, B2, B3 and the push-pull pyramid; lens blur buffer (B7); uniform with camera and ground (B8). The whole image's pipeline accumulates and bakes (preview in the output stage after a new tone result; export in the tone pre-pass and its finish); crops share the whole image's bake (`inherit_scene_bake`). Registry: `SceneNeeds { geometry }` (`scene: [Geometry]` in `mask_effects!`); no effect declares it, so the bake runs nowhere in the app or in render parity until P4. Contract test `scene_bake_uniforms_and_buffers_match_their_wgsl_declarations`. **Changed from the design while measuring:** (1) B3 decides edges per texel by two-means on log depth over the band radius (groups ≥ 10 % apart and explaining ≥ 85 % of the variance) instead of the min/max split, which cut the ground behind the subject by its texture (DSC09938 (1)); off edges coverage is a constant 0 or 1 (the side of the neighbouring edge's split) and both layers are the texel's own, so interpolation never blends in the other surface; the guided filter uses one window per texel, not the averaged second stage. (2) B7 measures the blur by the ratio of the gradient after two known blurs (1 and 3 texels, after Zhuo and Sim 2011) at gradient peaks within one surface, sky excluded, and fits the thin-lens form to each depth's 20 % quantile with capped weights; a profile-fit and the unblurred ratio both read wide blur as sharp. (3) Half floats in the pyramid are packed by hand (`pack2x16float` needs a capability). **Measurements** (desktop RTX 4070 Ti, headless, temporary dump since deleted): DSC09938 (1), fresh depth: edges on 6.5 % of texels; at full resolution the 50 % coverage contour lies a median 1.0 px from the photo's strongest edge, 66 % within 1.5 px, 90th percentile 6 px (outliers on fuzzy fur, the low-contrast crown and the hair clip, where the strongest edge is ambiguous; by eye the contour follows the silhouette everywhere); lens blur in focus at z 1.96 (the plush), near road σ 7 px, far buildings 6.5 px at full size; ground found (camera 0.30 above it, horizon just above centre). DSC08405 (1), migrated depth: edges follow wires and catenary, the far layer removes them. `k you ve.CR3`, migrated depth: the tree line's own edges read σ ≈ 8 px (≈ 21 px 10–90 %), but the fit gives 4.8 px there, as the migrated frame (R = 2.28) compresses distance; no ground found (grass slope, derived normals). Bake time without accumulation 2.6–2.9 ms at 1540×1027 (within §10). Memory ≈ 113 MiB at 1540 (layers 63, working image 17, scratch 25, inputs ~8), above §10's 64 MB estimate; ≈ 63 MiB at 1152. **Open:** S2's photo (not given), DSC05314 not run, Tab M11 time and memory (not connected), the thin-lens criterion on fresh depth of `k you ve.CR3`, the 1.5 px criterion holds for 66 % of contour samples by the automatic measure. |
| P3 | built; checks partly done | **Built:** `scene/lights.rs`, shaders `scene/lights_common.wgsl`, `shadow_raster.wgsl` (the first render pipeline), `visibility.wgsl`. L1: one `Depth32Float` cube per light that casts shadows (8 channels), 512² faces in Final and 256² in Draft or below the full-bake budget, faces that see no scene skipped; the mesh is the visible surface per texel (near where coverage ≥ ½, else far; sky not drawn), Draft draws it at half size. Instead of slanted skirts, a triangle across a silhouette is replaced by a **wall** behind the near outline, as deep as `SCENE_WALL_DEPTH` (0.1) of its depth: skirts self-shadowed the rim of a light behind the subject. L2: PCSS per surface (near and far layers, four lights per `rgba8unorm` layer), normal offset and slope bias raised against single-photo depth noise (1–1.5 texels), least filter 2 texels (a 1-texel filter showed the mesh's steps as dots). Keys: generation, position, face size, mesh level; visibility also Size. **Tier wiring** (this session): `GpuParams::with_scene_light_quality`; the UI renders sharp previews in Draft while a control or handle is dragged (`control_dragged`), the reduced drag preview always in Draft, and queues one output-stage render in Final when the drag ends (`refine_draft_scene_lights`). Exports use Final. **L3 built** (2026-11-06, later session): `shaders/scene/air_light.wgsl`, `scene/lights.rs` (`SceneAirLightRegion`, `encode_air_light`), `effect_inputs.rs` (`AirLightInputs`, per pipeline, with a placeholder), reader `scene_air_light` in `geometry.wgsl`. Per pipeline a texture array (`rgba16float`: layer 0 near and far depth of the ray's surfaces, then near and far shares per group of four lights) over the grid texels its pixels read, `march.x` pixels per texel (2 Final, 4 Draft), 48 or 16 steps even in control depth with a hash jitter of the grid position, 4 or 1 cube taps. The share is `Σ v·w / Σ w` with `w` = falloff along the ray × `dc` (so steps are even in control depth but weigh by linear depth). The reader is a joint bilateral upsample of the four grid texels around a pixel, each read on the near or far side whose depth is closer to the surface's, weight bilinear × `exp(−(Δln z / 0.05)²)`. Keys: each light's cube key and reach, the region and the march; Amount, colour and every media slider rebuild nothing. Every Relight light that emits goes into the bake whenever a medium reads it (`reads_air_light` in mask_params.rs), whether or not it casts shadows on surfaces. Lens blur fit copied into `SceneBakeUniforms.focus_fit` so effects need no storage buffer (`scene_lens_blur`). Texture count of the creative layout: **16** (15 + the air light; §4.6). **Measured:** S5 with fog (light behind the toy, Light glow 50) renders with no rim or lit fringe at the head and ears; 64-px tiles against the default tile size differ by at most 0.004 of a level (max over 8-px block means 6e-5). **Not done:** the synthetic box-and-sphere case, S3 (photo not given), grazing-light leak and DSC05314 acne checks, the Draft-to-Final brightness check by eye (the tiers estimate the same share, but nobody has compared them), timings (dropped). |
| P4 | built, S4 fixed | **Built:** Relight rewritten on `scene_pixel` (two surfaces, own normals with a Size-softened normal, inverse-square falloff, L2 visibility, sky unlit, flat plane without depth); M2 in `migrations/scene_depth_controls.rs` (Relief → `relief_scale`, `depth` → `light_depth`; sidecars and presets, preset schema 5, M4 extended); Relief control 10–300 %, default 100; labels unchanged, so no locale changes; `EffectReach::Pixel`. Bridge: textures 71/73/75/77 merged into one array; creative pass at **15** sampled textures (re-counted). Fixtures: four sidecar fixtures re-blessed with M2's values (`relief 20 → 33.48`, `depth 60 → 80.44`), new preset fixture `scene-controls` (Relight with an additional light, Fog start 30 for M3). **Fixed while checking** (this session): (1) the far surface's sky now comes from the bake (push-pull carries sky probability, GEOMETRY.y is the far surface's sky), so the far depth beside the sky is no longer lit as a surface (it drew S4's line on `relight-sky`); (2) texels beside an edge are layer samples of their side, so a crease (wall meeting floor) no longer fills its near layer from a surface 30 texels away (a lit strip on `relight`); (3) bridge: the old surface texture is built whenever the old scene shadow map is read, so Fog's Light glow from a Relight light no longer goes dark (`depth-effects`). Render parity re-blessed after review: 6 Relight cases changed as intended, 5 others below a level (≤ 0.002). **Photos:** S5 (`DSC09938 (1)`, light at depth 100): no fringe at the head and ears; the background beside the head is darker over ~70 px because the depth itself ramps there (0.39 → 0.95), not the shading. **S4 not met:** on migrated and on fresh depth the added light switches on within ~5 px at the tree line, against the photo's ~40 px ramp (fresh: x = 480, light 0 → 0.08 at y 670 → 675; photo 640 → 680), with dark patches under it. Cause: coverage at a defocused sky outline follows the depth model's step, as B7 reads the tree line nearly sharp, so the guided window stays small; wider guided windows were tried (colour and brightness) and failed (colour is not linear across sky, hills and field; brightness fits rarely took over). A fix needs a decision (§12 item 7). **S4 fixed** (later session, 2026-11-06; §12 item 7): (1) B2 widens the sky's band to `SCENE_SKY_BAND_RADIUS` 32 texels (the tree line's ramp in `k you ve.CR3` is ~23 working texels, and the model's outline wanders up to ~20 texels from it). (2) New pass `bake_scene_sky_colours` (bake_coverage.wgsl, before B3): each sky-band texel reads a line across the outline along the sky probability's slope, finds the photo's steepest step within 16 texels of the model's outline, walks out to where the step has flattened to 15 %, and takes F and B as 3-texel means just beyond those ends, with the texel's side (beyond the near end, in the ramp, beyond the sky end); stored in the NEAR/NORMALS layers until B4. (3) B3 averages those lines over 6 texels (Gaussian) and fits its guided filter to `α` from the averaged colour line within the ramp and 1 or 0 beyond its ends (by share), and to the old `1 − sky` only where no line was found (then, as before, only within 8 texels of the outline). Per-texel lines without averaging gave sharp-edged patches of light in low-contrast tree tops. (4) The edge kind passes from B3 to B4 (FAR, then NEAR/FAR `.w`); at a sky-only outline the far surface's samples are the sky (sky 1, d 1), so the model's tree-shaped bumps into the photo's sky stay unlit. (5) The colour split became multiplicative (§4.2): with the additive residual, texels with coverage just under 1 against the bright sky went negative and drew dark blotches. **Result** on fresh depth (`kyouve_fresh`, light at (36.8, 26.4), depth 86.7): no line or contour at the tree line at 33 % or 100 %; the light fades over the photo's own ramp; the largest step of added light per pixel fell from 0.21–0.56 to 0.07–0.09 of its range. The user's own sidecar light (depth −44): unchanged by eye. Left over: a faint veil in the sky beside the wispy, low-contrast tree tops at the top left, and a dark crescent beside a white bokeh ball at the tree line (the bokeh is sky-coloured, so it counts as sky). Render parity re-blessed after review: `relight-sky` changed visibly (its chart has no photo edge where its depth puts the sky; the old golden showed speckled light along the band), `relight` and `relight-lights` by ≤ 2.3 levels in band squares, `depth-effects`, `rain-wet-relight` and `relight-normals` below a level. **Parity cases added** (later session): `relight-silhouette-sky`, `relight-defocused-horizon` (the synthetic chart has no defocused photo edge, so its sky outline lies in the colour band), `relight-cast-shadow`, `relight-rim-behind`, `relight-eight-lights-shadows`, each on a generated 8-bit depth (subject in front of a receding ground and sky, `scratchpad/mkcases.py` in that session; the PNG is inside each case) and reviewed by eye. **Regression renders** (fresh depth, a Relight with shadows at (35, 45), depth 30, at 2400 px; `scratchpad/reg`): DSC05155 no contours in the sky and no new artefacts (the soft round patches under the canopy come from a no-depth Rain in that sidecar's Mask 4, which takes the old path and is shaped by its mask); DSC05314 no blotches on the fur or the pole; DSC08714 flat objects lit correctly, the chain's shadow falls on the ground. L2 does not depend on the tile size (DSC05314, 64-px tiles: 0.05 levels in 8-px block means). **Not done:** S2 and S3 (photos not given), user review. |
| P5 | built, awaiting review | **Built** (2026-11-06): the shared medium (`medium.wgsl`) reads a pixel as up to two surfaces (`MediumTarget`): each is fogged to its own depth and blended by coverage, so silhouettes and tree lines get no rim. Fog (`fog.wgsl`), Smoke's lighting (`smoke.wgsl`: each surface lit by the bake's L2 visibility, blended) and Rain (`rain.wgsl`: `rain_shade` once per surface, blended). Relight lights scatter in the medium in their own frame: the unshadowed closed-form integral over the 12 cells times L3's share at the surface (`MediumLight`, `medium_falloff_along`, `medium_phase`); Light Rays' lights and Glow/Neon emission stayed on the old path until P7. **M3** done: Fog `start` → `start_depth` in control depth (`migrations/scene_depth_controls.rs`, default 8 → 23, fixtures re-blessed: preset 30 → 56.49, sidecar 8 → 23.25; `older_readers` writes schema 5 for edits with a Fog). Fog and Smoke: `reach: Pixel`; registry needs `AirLight` (Fog, Rain) and `LightBake` (Smoke). Rain: slab sides at or past the nearest surface are control depths (`rain_side`, R7), slab geometry in the depth result's frame `z = 1 + R·d`, defocus from B7 (`rain_scene_blur`, via `scene_lens_blur`), drops hidden per surface. **Measured:** S5 (`DSC09938 (1)`, Fog on, Light glow 50, light depth 100): no rim or fringe at the head and ears; tiles 64 vs default: identical within 0.004 of a level. **Parity** (re-blessed after review): `fog` (silhouette lines, R2), `rain`, `rain-splashes`, `rain-wet-relight`, `glow-scene-light`, `neon-scene-light` (rain's drops differ, R7), `depth-effects`; new cases `fog-silhouette-sky`, `fog-glow-light-behind-subject`, `rain-occlusion-layers`. **Built 2026-11-07:** Rain's drops and splashes. Light for drops in the air is `scene_light_in_air` (the light bake's shadow cubes through `scene_point_visibility`, four taps, no blocker search), splashes use the surface's visibility (L2) and the fitted plane (B8): rows and cells are laid out as before from the distance below the horizon, with the camera's focal length instead of 0.4, and only on a surface that is ground. Rain's surroundings (`around`) read the working image (`scene_image`, a 3 × 3 Gauss–Hermite pattern of mip taps for a Gaussian of 8% of the longer edge). Rain without a depth result lives in an assumed frame (`rain_camera`), with the light bake's lights in a flat plane (`scene_light_camera`). **Measured on photos (1400–1600 px, fresh depth):** DSC08062 with Fog 80: no rim or bright line at building outlines or the crane; the one flaw is a faint ghost edge along the roofline of the right-hand building behind the "カニトップ" sign, where the model's outline and the photo's differ. **Not done:** DSC05168 (the photo is not in Demo) and the pillar-shaft check on DSC05155 (Relight at depth with Fog glow was not rendered). |
| P6 | built, awaiting review | **Built 2026-11-07:** Wet Surfaces rewritten on the scene bake (`wet_surfaces.wgsl`). Ground is per surface: its normal within about 20° of the fitted plane's (smoothstep 0.90–0.97 of the cosine) and its distance from the plane under 4–10% of its depth; each surface of an edge pixel is shaded alone and blended by coverage. Puddles live on the plane in camera heights (`scene_ground_position`, exact intersection of the view ray with the plane, footprint by finite difference), keep the fbm field, grain edge, soak and lens-matched ramps; the contact distance, terrain from rings and `WET_FAR_*` are gone (the sky alone is dry). Roughness is the guide's relative variance over three windows of 3 × 3 samples (smoothest of the three, so objects leave no rough ring); **its thresholds are the old ones and were not recalibrated on photos.** Reflections are traced (`scene_trace` in geometry.wgsl: Hi-Z in the working image's alpha, geometric steps from 1.5 working texels growing ×1.12, 48 steps, bisection to level 1, thickness 25% of depth, 1% self bias); the ripples tilt the plane's normal (`WET_RIPPLE_TILT` 0.12) and a miss reads the wide-footprint image where the ray left. Glints use the light bake (visibility per surface) and the emission grid. The pass has no environment grid any more (textures 69/70 are gone). **Measured:** DSC05155 (fresh depth, Wet Surfaces 75, puddles 55, reflections 90): the platform reflects the train, its red lights and the pillars; the platform darkens; the rest of the picture is unchanged. Parity case `wet-object-on-floor` added; the synthetic chart gives the plane no ground to reflect on, so it checks only that nothing breaks. **Not done:** `wet-floor-to-wall`, the S1 photo (never given), DSC09938's floor (no tendrils check), DSC04230 kerb and DSC07940 grass (no pasted-in puddles), the roughness recalibration. |
| P7 | built, awaiting review | **Built 2026-11-07:** (1) Light Rays' source is a light of the light bake (`SceneLightKind::LightRays`: at the farthest surface, reach 0.35 × Length × `2·focal_tan·(1+R)`), so Fog, Smoke, Rain and Wet Surfaces shadow it like Relight's; Light Rays and Relight lights use one list and one set of functions (`scene_light_at`, `scene_light_in_air`, `scene_light_on_surface` in `scene_lights.wgsl`). (2) Glow/Neon emission reads the bake's depth (`scene_emission_depth_at`) and lights each surface of a pixel with its own softened normal; its passes bind the bake's uniform and layers. (3) Deleted: `scene_surface.rs`, `scene_depth.rs`, `ground_plane.rs`, `scene_depth.wgsl`, `scene_shadows.wgsl`, `scene_analysis.wgsl`, the scene-map `FullImageMap` and its passes, bind groups, textures and budget entries, the scene shadow map pass and key, the `scene_ground` and `scene_depth_present` uniforms (the latter is `_padding`, keeping the next vector aligned), `SceneMapUse` and the registry's `scene_map` field, the `SCENE_LIGHT_CHANNEL` option, `SCENE_EDGE_SUPPORT` and `SCENE_DEPTH_GUIDE_SUPPORT`, T1's bridge fields in `GpuParams`, every `depth-rework-bridge` tag, and their layout contract checks. The creative layout binds 16 sampled textures (11, 24, 30, 27, 34, 36, 45, 83, 105, 112, 113, 114, 117 and the masks) — within the limit. (4) ARCHITECTURE.md (the `SHARED_SURFACE` row, the effect inputs row, the scene section) and DEVELOPMENT.md ("Adding an effect") describe the scene set. **Checks:** fmt, clippy, tests (layout contracts, render parity, formats), arch-check, migrations (33 markers, none due at v1.4.0) and deny pass. **Parity re-blessed** (backup `backup/render-parity-p6-before` in the session scratchpad): `rain`, `rain-splashes`, `rain-no-depth`, `rain-occlusion-layers`, `rain-wet-relight`, `glow-scene-light`, `neon-scene-light` (Rain's surroundings come from the working image, up to 20 levels in the top left of the synthetic chart), and by under 2 levels `depth-effects`, `fog-glow-light-behind-subject`, `wet-surfaces`; new case `wet-object-on-floor`. **Behaviour changes to name in the release notes:** without a depth result Glow and Neon light a plane facing the camera as before, but Rain and Wet Surfaces lights come from the flat plane's frame; Rain and Wet Surfaces are no longer offered in the picker. **Not done:** the migration markers were not re-checked against a release version (none is due). |
| P8 | not started | |
| P9 | not requested | |

---

## 12. Review of this plan (2026-11-06)

The plan was checked against the code before P1. Its direction holds: the four reported
failures share the structure in §2, and the legacy maths in §7 reproduces today's
frame exactly (`distance_from_stored` equals `d(i)` under fit (1, 0.115) and anchors
(1, 0); Relief is `mix(0.25, 6, p^1.5)`; the light depth migration is exact). Found
and corrected:

1. **No MoGe re-export.** The shipped ONNX has `points` (pinhole, square pixels:
   focal from x and from y agreed within 1.4 %, 2.7 % residual) and `scale`. Decision 3
   rewritten, M5 dropped.
2. **M1 contradicted its own acceptance.** Old maps are stored at up to 4096 px.
   Resampling them to 1540 would have changed Depth Range masks and the bridge's
   values. M1 now keeps the map's size and holds smoothing within half a level.
3. **Decided 2026-11-06 by the user: the texture limit, option (a).** Pack the bake
   into one `rgba16float` texture array and, under the bridge tag, merge old textures
   so P4–P6 fit; the per-phase count is in §4.6. (Rejected: (b), porting every
   creative-pass depth reader in one phase.)
4. **Measure the shadow cubes early.** The full working-size mesh is ~3.2 M triangles
   per cube face; on the Mali-G52 that may exceed §10's 25 ms per light. Time L1 on the
   Tab M11 at the start of P3, before P4 depends on it; a coarser shadow mesh is the
   fallback.
5. **P1's render-parity criterion cannot hold for files without normals** (§11 P1).
6. **Decided 2026-11-06 by the user: the air light is marched per pixel (§4.4 L3).**
   After P1 the user reported S5. The planned froxel volume (working/8, ~37 photo px)
   would have drawn the head's shadow in the fog in steps again. The user asked that
   it stay quick while moving and be high quality after moving and on output: Draft
   tier while dragging, Final when idle and on export.
7. **Decided 2026-11-06 (the user asked for the remaining work to be built):
   coverage at defocused sky outlines** comes from the photo's colours,
   `α = (I − B) · (F − B) / |F − B|²`, falling back to the guided fit of the sky
   probability where F and B are too alike. Built as described in §11 P4; F and B are
   read from the working image along each texel's line across the outline, not from
   B4's layers (which B3 feeds).
