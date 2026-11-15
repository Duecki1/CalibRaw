# Processing speed plan

Written 2026-11-14 from a read of the code, not from the older docs. Goal: a
slider drag and an export run only the work the edit needs, with the export
output unchanged and the preview showing the same picture.

Status marks: **done** (built and checked here), **open** (not built, with the
reason).

## 1. How a render runs today

A photo is processed in three stages (`ProcessingStage`), each a list of
compute passes over full-size 16-bit float textures:

| Stage | What it does | Runs again when |
| --- | --- | --- |
| Raw | highlight reconstruction, demosaic, chromatic aberration, colour denoise | white balance, denoise, demosaic, highlight settings change |
| Tone | tone statistics and guide, full-image maps (haze, image lights, contours) | calibration changes, or Raw ran |
| Output | scene adjustments, effects, display transform | every other edit |

The Output stage has three parts (`output_parts.rs`): Scene (exposure, tone,
masks, Texture/Clarity, blurs, Lens Blur), Creative (Glow, Fog, Relight and
the other creative effects) and View (colour mixer, grading, LUT, Vignette,
Grain). A preview restarts at the first part an edit reaches.

What already limits the work, and stays as it is:

- A drag renders the costly effects in the Draft tier, and on a slow GPU a
  second graph at half the edge follows the drag (`interactive_preview.rs`).
- A View or Creative control skips Lens Blur and everything before it.
- Passes of features that are off are skipped (dual demosaic, chromatic
  aberration, colour denoise, detail pyramid, Glow, blur, creative pass).

## 2. Why it is still slow

### 2.1 An Exposure drag with Lens Blur

Exposure is the first operation of the Scene part, and Lens Blur blurs the
toned image, so every later pass has to run again. That is inherent in the
order of operations. What was not inherent:

1. **Full-size passes that change nothing.** Whenever any pass after the
   scene tone pass was needed (a mask, Saturation, any effect), the chain ran
   three more full-size passes regardless of what the edit holds: the mask
   tone pass, the scene effects pass and a pass that only copies the result
   back to the first work texture. A Lens Blur on an otherwise plain photo
   paid for all three on every frame; an export paid for them on every tile
   of every pass over the photo.
2. **One program holds every effect.** `apply_creative_effects` and the first
   Blur stage dispatch by effect ID at run time, so the compiled program
   contains every creative effect, or every focus blur (the legacy Lens Blur
   among them), whatever the edit uses. Measured on an Adreno 830 before this
   plan: 1192 ms for the creative pass of one export tile, 14 ms when the
   program held only the edit's effects.
3. **Parameters uploaded that did not change.** Every stage of every preview
   uploaded the whole mask slot table, 428 slots of 1488 bytes (637 KB),
   three times per frame and per graph, even for a photo without masks.
4. **Lens Blur reads each pixel about eight times.** Its prepare pass, the
   sources of three local bins, the fill and the three map bins each compute
   the pixel's two surfaces and circle of confusion from the scene bake again.
   This is the largest remaining cost of a drag (about 80% of a Draft frame on
   a Pixel 7 Pro) and is not changed here; see 4.1.

### 2.2 Exports with many edits and masks

An export renders the photo in tiles, each with a halo, in up to four passes
over the photo: tone prepass, Lens Blur map prepass, Glow map prepass, final
tiles. Per tile and per pass:

1. The no-op passes and the all-effects programs of 2.1 run again.
2. **Every mask is rasterized on the CPU for every tile of every pass**, and
   each rasterization first cloned the whole mask stack (every mask's brush
   dabs and settings) to crop it to the tile: with L masks, L clones of L
   masks per tile.
3. Masks that nothing reads (switched off, zero opacity, an effect that is
   off) were rasterized and uploaded all the same.
4. Tiles overlap by their halo, so the masks and the GPU work cover
   ((core + 2·halo) / core)² times the photo: 2.25× to 6.25× on a computer's
   1024 px core. Phones already take the largest core their memory allows.
5. Lens Blur renders twice per tile when a Glow reads its map, and on a
   computer the Raw stage runs again in each pass. See 4.2.

## 3. What this plan changes

Each step keeps the numbers: the passes that still run read the same inputs
and compute the same values. Checked by rendering every parity case before
and after and comparing the 16-bit output files byte for byte (results in
section 5).

### Step 1: run only the scene passes the edit needs — **done**

`GpuParams::needs_local_tone_pass` (an active adjustment mask) and
`needs_scene_effects_pass` (Saturation, Vibrance, Texture, Clarity, a mask
with one of them, or Neon) decide the two passes. The copy back to the first
work texture runs only when a Blur stage follows, the only reader that needs
the image there; the creative part and the view node already read either work
texture. `SceneImage` records which texture holds the image.

Passes saved per render of the Scene part, and per export tile and pass:

| Edit | Before | After |
| --- | --- | --- |
| Only a creative effect (Fog, Glow, Relight) | 3 no-op passes | 0 |
| Lens Blur or Blur on a plain photo | 3 | 1 (the copy the blur reads) |
| Adjustment masks, no Saturation/Texture/Clarity | 3 | 1 (mask tone) |
| Saturation, Texture or Clarity, no blur | 3 | 2 |

### Step 2: programs that hold only the edit's effects — **done**

The creative pass and the first Blur stage are compiled per set of effects
(`effect_programs.rs`): the generated dispatch (`effect_wgsl.rs`) and the
`#ifdef EFFECT_<NAME>` blocks around each effect's call leave out the effects
the edit does not use. An effect that is left out has no active slot, and
with no active slot its code returns the colour unchanged, so the result is
the same.

- A preview that meets a set for the first time keeps rendering with the
  program that holds every effect, while the set's program compiles on a
  worker; the next render uses it. Nothing stalls longer than before.
- An export and the other full-quality renders compile the set before the
  first tile, as part of building the graph.
- Sets are kept for the session, per program template, so the fitted
  preview, the crop, the reduced preview and the export share them.

### Step 3: upload parameters only when they change — **done**

The mask slot table is uploaded only up to the slots in use and only when
those bytes differ from the last upload, like the three uniform blocks.

### Step 4: export masks — **done**

- The mask stack is cropped to the tile once, not once per mask.
- A mask that no active slot reads is not rasterized; its atlas layer is
  never sampled (the shaders skip inactive slots before reading their mask).

## 4. Open, with the reason

### 4.1 Lens Blur: read each pixel once

Store each pixel's two surfaces (colour × coverage, circle of confusion,
inverse depth) once and let the eight readers load them. It needs three more
full-size textures per Lens Blur (16-bit float would change the numbers, so
32-bit: 48 bytes per pixel), on phones that already run short of GPU memory
for Lens Blur, and it has to be timed on a phone to be worth it. Not built
without device timings (`CALIBRAW` stage timing, "GPU stage" lines).

### 4.2 Export: fewer passes over the photo

- Keep the image before the creative part from the Glow map prepass, so the
  final tiles do not render Lens Blur again. It needs the whole padded tile
  (the creative effects read the halo), 2 to 6 times the photo in 16-bit
  float; only possible with RAM to spare.
- Rasterize each mask once for the whole photo and cut tiles from it. The
  per-tile rasters are not bit-identical to a crop of a full raster (feather
  and grow run on the tile's grid), so the export would change slightly; it
  needs a decision and real photos to compare.
- Larger tiles on a computer (less halo overlap). Dehaze sums per tile, so
  its output moves by under one 8-bit level with the tile size; needs the
  same decision.

### 4.3 Adreno

- X-Trans homogeneity (`xtrans_markesteijn_homogeneity`) is about 500× slower
  on Adreno than on a computer, and one export tile is a single GPU
  submission that can run past the driver's 2 s limit. Both need the phone to
  verify a fix; not touched here.

### 4.4 White balance drags

White balance is applied before highlight reconstruction and demosaicing, so
a Temperature or Tint drag runs all three stages. Changing that changes the
picture, so it is not part of a speed change.

## 5. Checks

The byte-for-byte comparison of each step before and after was not
recorded while steps 1 to 4 were built. What has been checked since:

- 2026-11-15, desktop (Linux, Vulkan): every render-parity case and format
  fixture passes against the goldens without blessing, after steps 1 to 4
  and after the tier change in section 6.
- Contract checks (`layout_contract_tests.rs`) keep the shortcuts in step with
  the shaders:
  - `inputs_left_out_of_a_part_are_not_read_by_it`: what `output_parts.rs`
    leaves out of a part's comparison is read by no entry point of that
    part (lanes inside a member, Blacks, are kept by hand);
  - `every_effect_def_is_dispatched`: every `#ifdef EFFECT_<NAME>` is in a
    set some per-set program compiles (step 2);
  - `banded_entry_points_offset_their_invocations`: banded passes reach
    their invocation and workgroup only through `dispatch_id` and
    `dispatch_group`.

## 6. Bugs and wrong maths found on the way

- **Lens Blur rendered twice per drag of an unrelated control** (fixed
  2026-11-15). A change of tier restarted the output stage at the first part
  that reads it, so dragging a View or Creative control with Lens Blur on
  rendered Lens Blur in Draft on the first frame and in Final on release, and
  reallocated its textures both times. Each part now keeps the tier it was
  rendered in (`output_parts.rs`), and the tier-sized resources are prepared
  only by renders that run their readers (`prepare_tiered_effects`).
- **Shortcuts kept in step by hand** (fixed 2026-11-15): the scene effects
  pass named Neon instead of the effects of its pass, and the first Blur
  stage's per-set program listed its focus blurs by hand. Both follow the
  effect registry now.

## 7. Per-set programs never narrowed (fixed 2026-11-15)

Step 2's programs held every effect all along: `ShaderManager::register`
gave each composable module every effect def, and naga_oil adds an imported
module's defs to the importing shader's, so every set compiled every
`#ifdef EFFECT_*`. Modules are now registered without defs.

Adreno 830 (S25 Ultra), export creative pass, 24 tiles of 1940 px:

| Creative effects | All-effects program | Set's program |
| --- | --- | --- |
| Glow (DSC08062) | 3766 ms | 156 ms |
| Glow (UWU05122) | 6873 ms | 224 ms |
| Fog / Relight | 5183 / 5683 ms | 632 / 466 ms |
| Glow + Relight / Glow + Fog | 10.9 / 12.2 s | 1.5 / 1.9 s |
| Fog + Relight | 7.4 s | 20.5 s |

Two or more per-slot creative effects (`EffectPass::Creative`, in the slot
loop) compile far worse on Adreno, also with a `switch`, so such sets keep
the all-effects program (`has_several_creative_slot_effects`). DSC08062
export on the S25: 29.8 s → 21.4 s, first tile 17.9 s → 12.2 s without a
program cache. Desktop parity and a DSC08062 export byte-identical; the S25
export of DSC08062 is byte-identical too.

Open, measured on the S25 (export of DSC08062 after the fix: tone prepass
2.7 s, Lens Blur map 1.5 s, Glow map 4.8 s, final tiles 5.2 s):

- **Several per-slot creative effects** (Fog + Relight: 7.4 s): run each
  run of same-effect slots as its own pass. Exact in exports (32-bit work
  textures); previews would need 32-bit intermediates or accept f16
  rounding. Needs a decision.
- **Demosaic finish**: done, see 7.1. Previews still recompute the
  reference (2/3 of a zoomed preview's RAW stage); storing it there needs
  a 32-bit texture of its own (+16 B/px), which would shrink budget-bound
  phone and tablet previews and raise what a desktop full-resolution
  preview needs. Needs a decision.
- **The RAW stage runs once per prepass on a computer**: a desktop export
  of DSC08062 demosaics every tile four times (tone, Lens Blur map, Glow
  map, final tiles: 70 stages each), the S25 once (24 stages in the later
  passes). Worth finding out why before the other prepass work.
- **Lens Blur in the Glow map prepass** (4.2 s): the map adds only the
  tile core, so the composite (1.9 s, almost all the focus-slab gather) and
  gathers could run over the core alone: about 1.2 s. Dispatch bands offset
  rows only; needs an x origin.
- **Lens Blur twice per export** (Glow prepass and final tiles, 4.1 s
  each): keep the pre-creative image of each padded tile in RAM (~720 MB
  for this photo on a phone). Saves time after "Preparing", not during.
- `accumulate_defocus_map` (1.1 s) computes each pixel's surfaces once per
  map bin (3x); see 4.1.
- Fitted previews on the S25 (703 px) are dominated by `gather_defocus`
  (125 ms) and were not slow; opening is slow at larger previews (tablet,
  desktop) where the creative pass and Lens Blur scale with pixels.

### 7.1 Stored RCD reference (2026-11-15)

The Bayer finish passes read the RCD reference (`rcd_reference_at`, about
35 texture reads) at every tap: once at the pixel, 4 more for the
false-colour guard or 173 for frequency chroma, up to 48 for sensor
denoise, and once in the dual prepare pass. Full-quality graphs now store it once per pixel
(pass4.wgsl `bayer_rcd_reference`, override `RCD_STORED_REFERENCE`) in
highlight work A. That texture is 32-bit there and free once RCD has run,
so the stored value is exact and costs no memory. Dual demosaicing's
prepared image moves from highlight work A to tex1, which nothing reads
after the reference pass. Previews (f16) keep computing it, so their
output, memory and preview sizes are unchanged (`stores_rcd_reference`).

Exports are byte-identical: DSC08062 on the desktop with the Reference,
Frequency-domain chroma and Dual modes, with and without luminance denoise;
DSC08062 and UWU05122 on the S25. Every render-parity output is
pixel-identical to the one before the change.

| DSC08062 export | Before | After |
| --- | --- | --- |
| Desktop: demosaic finish per pass | 137 ms | 12 ms + 14 ms reference |
| Desktop: first tile (warm) | 3.06 s | 2.10 s |
| S25: demosaic finish (tone prepass) | 1180 ms | 115 ms + 90–100 ms reference |
| S25: tone prepass | 2.5–2.7 s | 1.6–1.7 s |
| S25: first tile (program cache) | 11.8–11.9 s | 10.1–10.5 s |

UWU05122 on the S25: tone prepass 2.5 → 1.6 s. Runs on the S25 slow down
by up to 50% in every stage when they follow another export (heat), so
compare runs made in alternating order.
