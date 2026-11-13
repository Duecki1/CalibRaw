# Preview performance plan

Status: P1 implemented (uncommitted); the rest is a proposal. Written 2026-11-13 against `ecff3d7c`
(branch `EffectsRework4`, clean tree). Measured on a Pixel 7 Pro (Mali-G710)
and an RTX 4070 Ti.

Problem reported: since the last updates the editor is slow on Android. With
Lens Blur on, every slider is unusable; Glow, Fog, Relight and others are slow
too. Expectation: only what a changed value affects should run again.

## 1. Measurements

Tool: a scratch bench (not in the repo) that loads a Demo RAW with its sidecar
and depth, builds the fitted preview proxy the app builds (`build_proxy`,
1280 px on Android), adds one effect and times the Output stage with
`stage_timing`. It is cross-compiled with `cargo ndk` and run on the phone from
`adb shell`, so the GPU timings come from the real driver without driving the
UI. Photo: `Demo2.5/DSC09938 (1).ARW` (fresh depth).

Output stage, GPU time per render, 1280×854:

| Effect (parity case settings) | Pixel 7 Pro | RTX 4070 Ti | Slowest pass on the phone |
|---|---|---|---|
| none | 9 ms | 0.19 ms | `apply_scene_tone_node` 4.5 |
| **Lens Blur, by depth** | **1754 ms** | 26.5 ms | `gather_defocus` 1592 |
| Glowing Area (`Glow`) | 75 ms | 7.6 ms | `apply_creative_effects` 65 |
| Relight | 55 ms | 1.6 ms | `apply_creative_effects` 45 |
| Glow, Star style | 44 ms | 2.7 ms | `build_styled_glow_streak` 15 |
| Fog | 41 ms | 2.5 ms | `apply_creative_effects` 30 |
| Smoke | 37 ms | 2.1 ms | `apply_creative_effects` 26 |
| Glow, Bloom style | 29 ms | 1.7 ms | `apply_creative_effects` 14 |
| Light Shafts (Light Rays case) | 19 ms | 0.9 ms | `apply_creative_effects` 7 |

While a control is dragged (parameter alternating every render, same pipeline):

| Drag | Pixel 7 Pro | Note |
|---|---|---|
| Relight light position | 60 ms (Draft 59) | light bake adds 3-5 ms; the creative pass is the cost |
| Relight Amount | 55 ms | no bake work |
| Fog Density | 42 ms | no bake work |
| Glow Bloom Amount | 30 ms | |
| Exposure with Lens Blur | ≈ 1754 ms | everything below reruns |
| Lens Blur at the half-size fallback (640 px) | **697 ms** | only 2.5× cheaper than 1280 px |

Lens Blur per dispatch on the phone (1280 px; 640 px in brackets):

| Dispatch | ms |
|---|---|
| map bin 3 gather | 538 (265) |
| map bin 4 gather | 551 (271) |
| map bin 5 gather | 115 (57) |
| local bin 2 gather | 287 (27) |
| local bins 0, 1 gather | 118 (30) |
| map accumulation | 93 (19) |
| composite, sources, fill | 70 (16) |

## 2. Causes

### C1. The Output stage reruns as one block (all effects)

`ProcessingStage` has three stages, and `affected_stage` sends every change that
is neither a Raw nor a calibration change to `Output`. `encode_output_stage`
then encodes, every time: scene preparation, tone, local tone, detail pyramid,
scene effects, **Lens Blur and Blur**, the Glow passes and map, the creative
pass, the view pass. So a change to HSL, colour grading, a LUT, grain, Glow
Amount or Relight Amount reruns Lens Blur although its input did not change.
This is the "only the changed values should run" problem.

The pass order in the Output stage (shaders):

1. `prepare_scene_node`: dehaze, temperature, exposure, local exposure, profile look
2. `apply_scene_tone_node`, `apply_local_scene_tone_node`, detail pyramid
3. `apply_scene_effects_node`: texture, clarity, saturation, vibrance, mask scene effects
4. Blur stage: Lens Blur (`defocus`), Blur, Tilt-Shift and the other mask blurs
5. Glow passes, styled Glow map and streaks, Pixelate blocks
6. `apply_creative_effects`: scene emission light, Fog, Smoke, Relight, Glow composites, Light Shafts
7. `apply_view_node`: colour mixer, hue, grading, view transform, point colour, LUT, film finish

Each stage reads only the output of the stage before and its own parameters.
Stage 4's result stays in its work texture while 6 and 7 run (6 writes the
other work texture, 7 writes the display textures), and Lens Blur's result
has its own texture (`defocus_result_tex`). So rerunning from stage 5, 6 or 7
needs no new memory.

### C2. Lens Blur's gather is too dense at preview sizes, and its map has a fixed size

The gather is a scatter-as-gather: each output texel reads every source texel
within a bin's reach, about 4 loads per tap. A bin is gathered at the pyramid
level where its starting radius is about 8 texels (4 for bin 0), which gives
reach ≈ 19 texels, about 1100 taps per texel.

- **The full-image map (bins 3 to 5) is 768 cells on the long edge whatever
  the pipeline's size** (`DEFOCUS_MAP_LONG`, capped only by the image's own
  pixels). Bin 4 is written at twice its source resolution
  (`MAP_SLAB_MIPS`), so it costs as much as bin 3. This is 1.2 s of the
  1.75 s, and it is why the half-size fallback barely helps: the map stays
  768 cells.
- **Local bins cannot go below level 0.** On a 1280 px preview (short edge
  854) bins 0 to 2 all land on level 0, so bin 2 gathers at full preview
  resolution with reach ≈ 16. Per output pixel, the local gather costs about
  1000 taps at preview sizes against about 60 at 6000 px exports. The
  algorithm's cost is lowest where it matters least.
- Radii under 3 texels normalise each tap with an 81-step loop
  (`defocus_spread`), twice per tap.
- Map bins have no occupancy skip; local bins do.
- Everything is rebuilt every render (C1), the map included, though the map
  depends only on the blur input, the depth and Lens Blur's own settings.

On Mali the gather is bound by texture loads. The RTX is 60× faster at it
against 47× for ordinary passes, so the desktop hides the problem.

### C3. One shader for every creative effect

All creative effects compile into one entry point (`apply_creative_effects`,
dispatch generated by `effect_wgsl.rs`). The driver allocates registers for
the heaviest path of all of them, so a pixel that runs only Fog runs at the
occupancy of Relight + Light Shafts + emission + Glow. Experiment on the phone
(temporary edit, reverted): with only the edit's effect in the pass, Relight's
creative pass went from **44.5 to 25.0 ms** and Fog's from **31.5 to 19.4 ms**
(about −40 %), with identical maths. `apply_scene_effects_node` and
`apply_view_node` are built the same way, with a smaller spread.

### C4. Long GPU batches block everything

A batch the GPU has started cannot be taken back (`PreviewDispatchGate`). A
1.75 s Lens Blur batch occupies the only queue, so the editor's own frames and
the next edit wait behind it. The full-resolution render already submits one
stage at a time for this reason; the preview does not.

### Not causes

- Scene and light bakes are cached correctly: a Relight light drag rebuilds
  only the light visibility (3-5 ms), and Fog and Relight value drags
  rebuild nothing. (The bench first showed a full bake rebuild per render,
  but that came from decoding two copies of the depth: the bake compares depth
  by `Arc` pointer, which the app keeps.)
- CPU side: building `GpuParams` and encoding take under 0.5 ms.
- The program cache: no recompilation seen during drags.

## 3. Plan

Ordered by gain per risk. P1, P3, P4 and P5 change no output (verify with
render parity; P4 may differ by compiler rounding, see there). P2 changes only
what is shown while a control is dragged.

### P0. Bench in the repo

Make the scratch bench an `xtask` command (`cargo xtask preview-bench`): a
RAW, its sidecar, a list of effect variants, a proxy edge; run on the desktop
or built for Android and run through `adb shell` with the timings pulled back.
It replaces guessing with a number per pass in a minute, without the UI. Each
step below reports its before/after on the Pixel 7 Pro.

### P1. Rerun only the parts a change reaches (no output change) — built

Status: implemented 2026-11-13 (`pipeline/gpu/output_parts.rs`), uncommitted.
Measured on the Pixel 7 Pro (1280×854, Output stage per drag step, before →
after): Lens Blur on and dragging HSL 1752 → 5.3 ms, Grain 1769 → 8.6 ms, Glow
1777 → 30 ms, Fog 1777 → 36 ms, Relight 1792 → 48 ms; Exposure with Lens Blur
unchanged (1751 ms); Fog alone 41 → 36 ms, Relight alone 55 → 48 ms. Checked
on the desktop: every render-parity case with each setting changed and
changed back, resumed renders bit-identical to whole ones.

Corrected from the first proposal:

- **Three parts, not four.** `Scene` (1-4: scene preparation, tone, scene
  effects, the blurs with Lens Blur, Wet Surfaces), `Creative` (5-6: scene
  emission, Glow passes and map, Pixelate blocks, the creative pass, Rain's
  air pass) and `View` (7). A separate Blur restart would need the Scene
  result, which lives in tex1, and tex1 is overwritten by the Glow passes and
  the creative pass whenever a blur ran: keeping it costs a full-size texture,
  and would save only the Scene passes (≈ 9 ms on the Pixel). Lens Blur's own
  settings and everything upstream still rerun everything (P2/P3).
- **Changes are found in the GPU pipeline, not the UI.** `dispatch_stage(Output)`
  compares the uniforms as uploaded, the mask slots in use and the resources
  the parameters name with those of the pipeline's last output render, each
  value counted by the shaders that read it: the view node alone reads the
  colour mixer, grading, hue, brightness, point colours and the LUT; a mask
  slot by the passes of its effect (`MaskEffect::passes`), an adjustment slot
  by Scene except its view-only fields; everything else by Scene. This covers
  every preview graph (fitted, zoomed crop, reduced drag preview, navigation)
  and every edit path (sliders, masks, presets, card switches, LUTs) without
  classifying controls in `calibraw-core` or changing how the UI queues stages.
- **What the comparison cannot see is tracked explicitly.** The RAW and tone
  stages, exports, raw tile and scene region uploads and Remove patches forget
  the last render. Uploaded mask layers start at the first part whose active
  slots read them. A crop forgets when it inherits tone statistics from a newer
  full-frame tone stage, and compares the build of the full frame's Lens Blur
  and Glow maps it shares.
- **Rain and Wet Surfaces** run a pass after the creative pass that overwrites
  the creative input, so with them on only View restarts are possible.

Exports render every part, as before.

Expected: with Lens Blur on, dragging HSL, grading, point colour, LUT amount,
Grain or Vignette runs the view pass only (≈ 3 ms on the Pixel); Fog, Relight,
Glow, Light Shafts, Pixelate and the other creative effects run Creative + View
(≈ 15-50 ms). Toggling an effect on or off, or moving an amount across zero,
reruns everything once.

### P2. Lens Blur Draft tier while dragging (output changes during drags only)

The Draft/Final split the light bake already uses (`SceneLightQuality`): Draft
while a control or handle is dragged, Final when idle and on export.

- Draft gathers every bin one level coarser: starting radius ≈ 4 texels
  (2 for bin 0), so about a quarter of the taps over a quarter of the texels.
- The map is sized from the pipeline's long edge (its 768 cells at a 6000 px
  export are 8 px; at a 1280 px preview they are 1.7 px) and halved in Draft;
  bin 4 is written at its source resolution in Draft.
- Expected: map bins about 16× cheaper, local bins 4-16×; roughly
  1754 → 100-150 ms at 1280 px, and the half-size fallback then scales too.
  Numbers to confirm with P0.
- Final, exports and parity: bit-identical.

### P3. Exact Lens Blur savings (no output change)

- Occupancy skip for map bins, as local bins have it (a skipped group writes
  zeros, and the composite reads a slab's fill only where the slab covers).
- Cache the map: it depends on the blur input, the depth and Lens Blur's
  settings, so with P1 a pipeline keeps it until one of those changes.
- Normalisation for radii under 3 texels (`defocus_spread`): precompute the
  lattice sum once per source per bin pass, not per tap.
- Load order in the gather: read `extra` first and skip the other four loads
  where it is empty (partly done; `behind` is still loaded every tap).

### P4. One creative program per set of effects (no intended output change)

Generate the creative module per set of creative effects the edit uses: the
slot dispatch lists only those, and the fixed calls (emission, Glowing Area,
styled Glow, Light Shafts) are left out when absent. Build the variant in the
background the first time a set appears and render with the general program
until it is ready, so adding an effect never stalls. Keep variants in the
existing program template and pipeline cache. Measured gain on Mali about 40 %
for Relight and Fog. Compiler rounding may differ in the last bit between
variants: check parity and accept only bit-identical results, or ask before
re-blessing. Extend the same idea to `apply_scene_effects_node` and
`apply_view_node` if P0 shows a gain.

### P5. Short GPU batches (no output change)

Submit the Output stage in parts (after Blur and after Creative, and Lens
Blur per bin in Final) and check for a newer edit between parts, as the
full-resolution render already does between stages. A drag then never waits
behind a full Final Lens Blur, and the editor draws its frames in between.

### P6. Per-effect costs after P4

Profile again with P0 and decide per effect: Glowing Area's creative cost
(65 ms on the Pixel, mostly the slot effect and the core), Relight's
`scene_pixel`/`scene_split`/soft normal per surface, Fog's `medium.wgsl`
march, styled Glow streaks (15 ms). Any change of algorithm or look is a
separate, approved change.

## 4. Expected result on the Pixel 7 Pro (1280 px)

| Situation | Now | After P1 | After P1-P4 |
|---|---|---|---|
| Lens Blur on, drag HSL/grading/LUT | 1754 ms | ≈ 12 ms | ≈ 12 ms |
| Lens Blur on, drag Fog/Relight | ≈ 1790 ms | ≈ 45-55 ms | ≈ 30 ms |
| Lens Blur on, drag Exposure | 1754 ms | 1754 ms | ≈ 100-150 ms (Draft), Final after release |
| Relight on, drag Relight | 55-60 ms | 50-55 ms | ≈ 35 ms |
| Fog on, drag Fog | 42 ms | 38 ms | ≈ 30 ms |

All figures after P1 are estimates from the measured pass times; P0 measures
each step.

## 5. Decisions

1. P2 shows coarser bokeh rims while a control is dragged (the Final look
   returns on release, as Relight's light bake does). Acceptable?
2. P0 adds a bench command to `xtask` (tooling, not a test). Fine to check in?
3. Order: P1 → P3 → P2 → P4 → P5 → P6, or another?
