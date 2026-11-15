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
