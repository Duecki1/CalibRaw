# Panorama merge plan

Status: implemented, committed as `d70e204e` ("Basic panorama merge") on 2026-11-21,
with a second pass on seams and lens refinement after review on real frames (§12,
"Second pass"). §12 records where the implementation differs from this plan and what
has been verified.

Panorama merge joins two or more overlapping RAW frames into one floating-point
TIFF master, as HDR merge and focus stacking already do from the library's
Merge menu. It answers the open ROADMAP item "Panorama and focus alignment".

This plan records what other tools do (with sources), what CalibRaw already
has, the design, and the order of changes. Sections marked **Decision** are
proposals to review; **Open question** items need the owner's call.

## 1. What CalibRaw already has

| Piece | Where | Reusable for panoramas? |
|---|---|---|
| Develop a RAW to linear camera RGB, tiled on the GPU, with hot pixels, Lensfun correction, one shared white balance and optional AI denoise | `calibraw-gpu/src/pipeline/raw_merge.rs` (`merge_sources`, `render_camera_rgb`, `sensor_footprint_peak`) | Yes, once the per-source develop is split from the registration loop (§6.1). |
| Registration | `calibraw-core/src/pipeline/alignment.rs` | No. It is dense Lucas–Kanade on a pyramid with an exhaustive search of ±1/8 of the frame, and `ensure_plausible` rejects a centre shift above 15 %. A panorama step moves the frame by 50–80 %. The Catmull–Rom `resample` is reusable (§6.2). |
| Laplacian pyramid (Burt–Adelson kernel, `reduce`, `expand`, coverage reach) | `calibraw-core/src/pipeline/focus_stack.rs` (`Plane`, `Texel`) | Yes, for multi-band blending (§5.10). |
| Cube-root encoding of linear camera RGB for fusion | `focus_stack.rs` | Yes, the blend domain (§5.10). |
| Exposure from metadata (`exposure_scale`) | `calibraw-core/src/pipeline/hdr.rs` | Yes, the starting point for gain compensation (§5.8). |
| Focal length to 35 mm equivalent | `PerspectiveCorrection::known_focal_length_35mm` (`geometry/perspective.rs`), Lensfun `camera_crop_factor` | Yes, the initial focal length in pixels (§5.4). |
| Lens distortion map, corrected → sensor | `LensGeometryMap::source_position_for_raster` (`geometry/lens_map.rs`) | Yes, composed into the single warp (§5.11). |
| Memory-or-disk allowance for intermediates (RAM/8) | `calibraw-gpu/src/pipeline/export/intermediate.rs` (`IntermediateMemory`) | Yes, for the developed frames (§7). |
| Library Merge menu, options dialog, worker, toolbar task, save beside the source or in the Android library | `calibraw-ui/src/ui/library/merge.rs`, `actions.rs` | Yes, with a third `MergeKind` (§6.4). |
| Strip-by-strip TIFF writing, BigTIFF, ExtraSamples | `tiff` 0.11.3, already in `Cargo.lock` (`write_strip`, `new_big`, `extra_samples`) | Yes, so the master never has to fit one buffer (§5.12). |

Frames arrive upright: the rawler loader applies EXIF orientation at decode
(`Geometry::to_flips`), so a portrait-held sequence has portrait frames. This
matters for straightening (§5.6) and should be confirmed with a portrait set.

Limits that shape the design: a master must stay within `MAX_RAW_EDGE`
(32,768 px) and `MAX_RAW_PIXELS` (210 MP), or CalibRaw cannot open it again.
No merge has render-parity coverage today.

## 2. How other tools do it

| Tool | Licence | What it does (source) |
|---|---|---|
| OpenCV `stitching` (4.x) | Apache-2.0 (GPL-3 compatible) | Brown & Lowe's pipeline. `Stitcher::create(PANORAMA)` uses registration at 0.6 MP, seam estimation at 0.1 MP, compositing at full resolution, ORB features, `BestOf2NearestMatcher`, `HomographyBasedEstimator`, `BundleAdjusterRay`, horizontal wave correction, `SphericalWarper`, `BlocksGainCompensator`, `GraphCutSeamFinder(COST_COLOR)` and `MultiBandBlender` (`stitcher.cpp`). The `stitching_detailed` sample defaults to graph-cut colour seams, gain-blocks exposure and multi-band strength 5. |
| Hugin (cpfind, autooptimiser, vig_optimize, nona) | GPL-2.0-or-later | cpfind finds keypoints at half scale, keeps them spread with a 10×10 bucket sieve, matches through a k-d tree, and verifies with RANSAC whose model is a homography for HFOV < 65° and roll/pitch/yaw otherwise ("rpy … the preferred mode if a calibrated lens is used, or the HFOV could be read from the EXIF data"). vig_optimize fits exposure (Ev), white balance (Er, Eb), the EMoR response and a radial vignetting polynomial (Va = 1, Vb, Vc, Vd) from overlaps. |
| Enblend | GPL-2.0-or-later | Multi-resolution spline. Seams from the nearest-feature transform by default, graph cut optionally ("less likely to cut through visible structures", more memory and time). Floating-point images blend in the IDENTITY colour space by default. Its reduce is alpha-aware: uncovered samples are left out and the result is normalised by the summed alpha (`pyramid2.h`, as copied into Hugin). The halo of an `L`-level pyramid is `2^(L+1) − 4` pixels. |
| darktable, RawTherapee, ART | GPL-3 | No panorama merge. |
| Rust crates | — | `akaze` 0.7.0 (MIT) was last released 2020-06-23 and depends on `image` 0.23 and `ndarray` 0.13: not acceptable. `imageproc` (MIT, maintained) has corners but no scale-invariant descriptor. No pure-Rust SIFT crate was found. |

Background papers the code will cite: Brown & Lowe, "Automatic Panoramic
Image Stitching using Invariant Features" (IJCV 2007); Lowe, "Distinctive
Image Features from Scale-Invariant Keypoints" (IJCV 2004; US patent
6,711,293 expired March 2020, as OpenCV's `sift.dispatch.cpp` notes);
Brown, Hartley & Nistér, "Minimal Solutions for Panoramic Stitching" (CVPR
2007); Burt & Adelson, "A Multiresolution Spline With Application to Image
Mosaics" (1983); Kwatra et al., "Graphcut Textures" (SIGGRAPH 2003);
Boykov & Kolmogorov, "An Experimental Comparison of Min-Cut/Max-Flow
Algorithms" (PAMI 2004).

**Decision:** follow Brown & Lowe as OpenCV implements it. Take the robustness
details from Hugin (rotation-model RANSAC, spread-out keypoints) and the
alpha-aware pyramid from Enblend. Port code from OpenCV (Apache-2.0) where it
is a direct fit, credited in `THIRD_PARTY_NOTICES.md` under "Adapted source
and algorithms". Hugin and Enblend are GPL-2-or-later and may be adapted too,
but only the parts named here are needed.

## 3. Scope

Version 1:

- Single-row and multi-row panoramas from one camera body, hand-held or on a
  tripod, rotating about roughly one point.
- Projections: Auto, Spherical, Cylindrical, Perspective.
- Always cropped to the largest covered rectangle; there is no switch for it.
- The same develop options as the other merges: lens correction, hot pixels,
  AI denoise.
- Desktop and Android. iOS is not planned.

Later, not in version 1 (§10): HDR panoramas (bracket per position),
Boundary Warp, filling the edges, parallax-tolerant mesh warps, estimating
vignetting without a lens profile, estimating distortion without a lens
profile.

## 4. Pipeline overview

```
for each source (one at a time)          ── existing develop, split out (§6.1)
    decode → hot pixels → peak → Lensfun → shared WB → [AI denoise] → camera RGB
    stage full frame + clip mask (RAM or disk, §7)
    registration image: undistorted luminance, long edge 2048, cube-root encoded
    SIFT keypoints + descriptors                       (§5.1)
match: candidate pairs → ratio test → RANSAC          (§5.2–5.3)
graph: largest connected component, else fail          (§5.3)
initial rotations: max spanning tree, focal            (§5.4)
bundle adjustment: rotations + focal, robust, cleaned (§5.5)
straighten: wave correction                            (§5.6)
projection + canvas + size cap                         (§5.7)
exposure: metadata gain → overlap gain solve           (§5.8)
seams: low-res graph cut over Voronoi start            (§5.9)
composite in strips: warp → masked multi-band blend    (§5.10–5.11)
crop, convert to working Rec.2020, write TIFF strips   (§5.12)
```

Everything up to the seams runs on downscaled data. Only the final
compositing reads full-resolution frames, once each per strip they touch.

## 5. Algorithms

### 5.1 Features

**Decision:** SIFT, ported from OpenCV 4.x `modules/features2d/src/sift.dispatch.cpp`
and `sift.simd.hpp` (Apache-2.0, which retains Rob Hess's BSD notice) into
`calibraw-core/src/pipeline/panorama/features.rs`. The scalar path only; the
SIMD dispatch is not ported. OpenCV defaults: 3 octave layers,
contrast threshold 0.04, edge threshold 10, σ 1.6. Descriptors are 128 floats,
RootSIFT-normalised (L1-normalise, square root) for better L2 matching.

Why not ORB (OpenCV's Stitcher default): ORB is binary and not scale-space,
and it is weaker on foliage, water and rock, which landscape panoramas are
full of. AKAZE has no maintained Rust implementation (§2).

Input: the registration image. Exposure-normalised luminance
`(R + 2G + B)/4` of the lens-corrected frame (distortion removed through
`correct_lens_distortion` at that size, so keypoints are in undistorted
pinhole coordinates), downscaled to a long edge of 2048 px and cube-root
encoded as the focus stack does. Pixels whose footprint peak clips, or that are
nearly black, are masked so no keypoint sits on them. Keypoints are spread
with cpfind's sieve (10×10 buckets, best responses per bucket) and capped at
about 4,000 per frame.

### 5.2 Matching

- Candidate pairs: every pair when there are few frames (≤ 12). Above that,
  Brown & Lowe's preselection: one k-d tree over all descriptors, each feature's
  4 nearest neighbours vote for image pairs, and each image is matched with
  the 6 images it shares the most votes with, plus its neighbours in file-name
  order.
- Per pair: approximate nearest neighbours with a best-bin-first k-d tree
  (cpfind uses 200 search steps), Lowe's ratio test at 0.8, and the
  two-direction union OpenCV's `CpuMatcher::match` uses.
- The k-d tree is written in `panorama/kdtree.rs` (about 150 lines). No crate
  is added for it.

### 5.3 Geometric verification

- **Model:** with a known focal length (EXIF focal length and a Lensfun crop
  factor), a 2-point rotation RANSAC (Brown, Hartley & Nistér): the rotation
  between two bundles of rays, solved in closed form from two
  correspondences. Without one, a 4-point homography RANSAC. This mirrors
  cpfind's `auto` mode, which uses its rotation model when the HFOV is known.
- **Inlier threshold:** a few pixels at registration scale, followed by a
  least-squares refit on the inliers.
- **Accept a pair** when `inliers > 8 + 0.3 · matches` (Brown & Lowe, as
  OpenCV computes `confidence` with a threshold of 1).
- **Connectivity:** frames that do not join the largest connected component
  make the merge fail, naming them. OpenCV's `leaveBiggestComponent` drops
  them silently, which would leave the user wondering where a frame went.

### 5.4 Initial estimate

- **Focal length in pixels:** from `known_focal_length_35mm`, converted with the
  sensor diagonal. Without a crop factor, OpenCV's `estimateFocal`
  (`autocalib.cpp`, focal lengths from pairwise homographies, median), and the
  2-point RANSAC is not available for that merge.
- **Rotations:** a maximum spanning tree weighted by inlier count, walked
  breadth-first from its centre, composing pairwise rotations (OpenCV
  `HomographyBasedEstimator`, `findMaxSpanningTree`, `CalcRotation`).

### 5.5 Bundle adjustment

The ray cost of OpenCV's `BundleAdjusterRay`: for each inlier, the distance
between the two unit rays, scaled by `sqrt(f_i f_j)`. Parameters are an
axis-angle rotation per frame and the focal length. The reference frame's
rotation is fixed for the gauge.

- **One focal length** for all frames when their EXIF focal lengths agree, as
  they do from one lens setting. Otherwise one focal length per frame.
- **Solver:** dense Levenberg–Marquardt with a Huber loss, in the style of
  `alignment.rs`'s `refine`. At 4 parameters per frame, 40 frames give a
  160×160 system. It is a separate solver, because the parameterisation
  differs; the Gaussian elimination is not worth sharing.
- **Cleaning:** after convergence, drop matches with residuals above 3σ and
  refine again. This is Hugin cpclean's statistical idea.
- **Reject** a solution whose RMS ray error at output scale stays above a few
  pixels: the frames do not share a projection centre, for example from
  strong parallax.

### 5.6 Straightening

OpenCV's `waveCorrect`, with the direction chosen like
`autoDetectWaveCorrectKind`: horizontal when the frame centres span more
horizontally, vertical otherwise. Horizontal correction assumes each
camera's x-axis is level. Frames are decoded upright (§1), so this holds for
landscape and portrait sequences alike.

### 5.7 Projection and canvas

- **Projections:** Spherical (equirectangular), Cylindrical, Perspective
  (rectilinear), each a pair of forward and inverse functions in
  `panorama/projection.rs`, modelled on OpenCV's `warpers_inl.hpp`.
- **Auto** (CalibRaw's own rule, not taken from a source): Perspective
  while every frame's corner rays stay within about ±55° of the panorama
  centre, Cylindrical while the vertical extent stays under about 100°, and
  Spherical beyond. The thresholds are tuned on real sets during
  implementation.
- **Scale:** one canvas pixel per `1/f` radian at the centre, with `f` the
  median focal length (OpenCV's `warped_image_scale`). The centre then keeps
  the sources' resolution.
- **Extent:** frame outlines traced in the projection (OpenCV's
  `detectResultRoi` traces borders, not corners only).
- **Size cap:** the canvas is scaled down uniformly to fit, in order:
  `MAX_RAW_EDGE`, `MAX_RAW_PIXELS`, then the memory allowance (§7). The
  result reports the scale, and the UI shows a notice when it is below one,
  as it already does for a missing lens profile.

### 5.8 Exposure

1. **Metadata.** Each frame is divided by its `exposure_scale` relative to the
   reference, as HDR merge does, so an auto-exposed sequence starts aligned.
2. **Overlap gains.** OpenCV's `GainCompensator` objective: for every
   overlapping pair, mean luminance `I_ij` over the shared pixels and count
   `N_ij`. Minimise
   `Σ N_ij [ (g_i I_ij − g_j I_ji)² / σ_N² + (1 − g_i)² / σ_g² ]`, with OpenCV's
   weights (`alpha = 0.01`, `beta = 100`). Means are taken over linear
   luminance at registration scale on warped frames, without clipped or black
   pixels. One gain per frame: white balance is already shared, so a
   per-channel gain would only chase noise.
3. **Vignetting** is left to Lensfun. Without a profile, multi-band blending
   hides most of the low-frequency step. Fitting Hugin's vignetting
   polynomial is in §10.

OpenCV's `BlocksGainCompensator` (32×32 blocks) is not used. It is meant for
8-bit JPEGs with unknown response and vignetting. With linear RAW data and a
lens profile it would mostly flatten real light falloff across the scene.

### 5.9 Seams

At about 0.25 MP of canvas (OpenCV uses 0.1 MP; frames from 45 MP bodies
deserve more):

1. **Initial labels:** a Voronoi split by distance from each frame's edge, so
   every pixel starts in the frame where it lies deepest. This is Enblend's
   nearest-feature-transform idea.
2. **Graph cut:** a cut between each overlapping pair, pairwise and in order,
   as OpenCV's `PairwiseSeamFinder` does. The cost combines colour and
   gradient differences (OpenCV's `COST_COLOR_GRAD`, after Kwatra et al.) of
   cube-root encoded values. A pixel clipped in one frame and measured in the
   other costs the clipped side more, so seams prefer the measuring frame.
   Moving subjects (people, waves) show up as high colour difference, so the
   cut goes round them. This is panorama "deghosting", and there is no
   separate option for it.
3. **Max-flow:** port OpenCV's `GCGraph` (Boykov–Kolmogorov) to
   `panorama/maxflow.rs`.

The label map is upsampled with nearest neighbour to full resolution, and
blending softens the edges (§5.10).

### 5.10 Multi-band blending

**Accumulation**, as OpenCV's `MultiBandBlender::feed`: each frame adds
`L_i(level) · G(w_i)(level)` to one canvas pyramid and `G(w_i)` to a weight
pyramid. The result is normalised level by level and collapsed. It does not
depend on frame order, as the focus stack doesn't.

**Alpha-aware reduce** (Enblend): a frame's uncovered canvas pixels are
left out of its reduce and each coarse sample is normalised by the covered
weight. Without it, OpenCV's reflected borders would bleed into the coarse
bands near frame edges.

**Domain:** cube-root encoded, exposure-normalised linear camera RGB, decoded
after collapse. This is the focus stack's choice. Enblend blends floats in
IDENTITY, but on scene-linear HDR data identity rings visibly around bright
sources, and the cube root keeps the ringing small in shadows. As in the
focus stack, each pixel's luminance is finally limited to the range the
covering frames measured there.

**Band count:** OpenCV's rule,
`bands = ceil(log2(sqrt(area) · strength / 100)) − 1` with strength 5,
capped by the pyramid depth `focus_stack.rs` allows.

**Code:** move `Plane`, `Texel`, `reduce` and `expand` from
`focus_stack.rs` to `pipeline/pyramid.rs`, so focus stacking and panoramas
share them. That extraction removes duplication. Add a masked `reduce` beside
the existing one. The focus stack's output must stay bit-identical.

### 5.11 Compositing in strips

The canvas is composited in horizontal strips, which the TIFF is written in
(§5.12).

- **Halo:** Enblend's `2^(L+1) − 4` for `L` levels, plus the Catmull–Rom
  support.
- **Alignment:** strip boundaries snap to multiples of `2^L` canvas rows (as
  OpenCV snaps ROIs), so every strip builds the same pyramid samples. The
  output then does not depend on strip height.
- **Per strip:** for each frame whose outline meets the strip, warp its part
  of the strip plus halo, build its masked Laplacian pyramid, and add it to the
  strip's accumulator.
- **Warp**, one resampling per output pixel: canvas → ray (inverse
  projection) → frame rotation and focal length → undistorted frame pixel →
  `LensGeometryMap::source_position_for_raster` → sensor pixel → Catmull–Rom
  on the staged frame. Distortion is corrected in this warp, not in
  `MergeCamera::finish`, so no second resampling softens the panorama.
- **Downscaling:** when the canvas scale is below about 0.7 of the source,
  frames are prefiltered (box-reduced by the integer part of the factor)
  before Catmull–Rom, so a capped panorama does not alias.
- **Parallelism:** with rayon within a strip, as the existing merges do.
- **Cancellation:** checked between strips and frames.

### 5.12 Crop and output

- **Auto crop (always):** the largest axis-aligned rectangle inside the
  covered area, found on the seam-resolution coverage mask with the
  histogram-stack maximal-rectangle algorithm, shrunk by one low-resolution
  pixel. Only rows and columns inside the crop are composited. The develop
  pipeline has no alpha, so a master never has uncovered pixels.
- **Colour:** each strip is converted to working Rec.2020 with the
  reference's camera matrix and white balance (`camera_to_working_rec2020`).
- **Writing:** strips go out through `tiff::encoder::TiffEncoder::write_strip`
  with the existing linear Rec.2020 ICC profile. 210 MP of RGB f32 is 2.5 GB,
  within classic TIFF's 4 GB, so BigTIFF is not needed while the cap holds.
- **Name:** `{stem}-Pano.tif`.

## 6. Code changes to existing modules

### 6.1 Split the per-source develop out of `merge_sources`

`raw_merge.rs::merge_sources` develops a source and registers it in one loop.
A panorama needs only the develop part.

- Extract `develop_source(...) -> DevelopedSource { rgb, peak, exposure, light }`
  plus `MergeCamera` setup. `merge_sources` keeps its loop and calls it.
- This serves a real additional caller.
- HDR and focus outputs must stay bit-identical. Check by merging the same
  real sets before and after and comparing the TIFFs byte for byte.

### 6.2 Generalise `alignment::resample`

It writes a grid the size of its source. Give it separate output dimensions,
plus a coverage test the warp supplies, so the panorama warp can call it.
This serves a real additional caller. `warp` and `correct_lens_distortion`
keep their behaviour.

### 6.3 New module `calibraw-core/src/pipeline/panorama/`

| File | Contents |
|---|---|
| `mod.rs` | `PanoramaBuilder`: frames added one at a time (registration image, focal length, exposure, staged-frame handle); `solve()` → `PanoramaLayout`; `composite(layout, frames, sink)` |
| `features.rs` | SIFT port |
| `kdtree.rs` | best-bin-first k-d tree |
| `matching.rs` | pair selection, ratio test, RANSAC (2-point rotation, 4-point homography) |
| `camera.rs` | rotations, max spanning tree, focal estimate, bundle adjustment, wave correction |
| `projection.rs` | `Projection` enum and forward/inverse maps, canvas extent |
| `exposure.rs` | gain solve |
| `seams.rs`, `maxflow.rs` | Voronoi labels, pairwise graph cut |
| `blend.rs` | masked multi-band accumulator over a strip |
| `crop.rs` | largest covered rectangle |

Typed coordinate spaces, per AGENTS.md: `SensorPx`, `UndistortedPx`,
`Ray` (unit `[f64; 3]`) and `CanvasPx`, so a distorted coordinate cannot be
projected as an undistorted one.

`calibraw-core` stays free of GPU and UI dependencies, and `arch-check` is
unaffected.

### 6.4 GPU and UI glue

- `calibraw-gpu/src/pipeline/panorama.rs`: `merge_panorama(device, queue,
  paths, options, denoise, prewarm, progress, write)` develops through
  `develop_source`, stages frames (§7) and drives `calibraw-core`'s panorama
  module. It streams strips to a writer callback instead of returning a
  `MergedImage`.
- `MergeOptions` keeps the develop switches. Panorama choices go in their own
  type, `PanoramaOptions { projection: ProjectionChoice }`, rather than more
  fields that two of three merges ignore. `align` and
  `reject_motion` do not apply to panoramas.
- UI (`ui/library/merge.rs`, `actions.rs`):
  - `MergeKind::Panorama`: title "Panorama", suffix `Pano`, glyph
    `egui_phosphor::regular::PANORAMA`.
  - Dialog rows: Lens correction, Projection (segmented: Auto / Spherical /
    Cylindrical / Perspective), Remove hot pixels, AI denoise. No
    explanatory text, per the UI rules.
  - `ErrorKind::Panorama` ("Panorama merge failed").
  - The menu tooltip follows the others' pattern.
- `save_merged` takes a write closure instead of `&MergedImage`, so HDR and
  focus write as before and the panorama streams.
- `MergeChoices` are not persisted ("until CalibRaw quits"), so there are no
  settings or migration changes.
- **Locales:** every new string in all 16 catalogs.

## 7. Memory and time

- **Developed frames:** RGB f32 plus a 1-byte clip mask per pixel. 10 frames
  of 45 MP are 5.9 GB, so frames are kept in RAM while they fit
  `IntermediateMemory` (RAM/8). Beyond that they are staged as raw files in
  `file_ops::staging_directory` of the output folder and read back by row
  range during compositing. `IntermediateMemory` moves from `export/` to
  `pipeline/intermediate.rs`, `pub(crate)`. This serves a real additional
  caller.
- **Compositing:** the strip height is chosen from the remaining allowance:
  accumulator `≈ 4/3 · width · (strip + 2·halo) · 16 B`, plus one warped frame
  part.
  - Desktop, with `L = 7` (halo 252) and a 30,000 px wide canvas: 512-row
    strips take about 650 MB.
  - Android: the RAM-scaled allowance lowers the strip height first, then the
    canvas scale (§5.7).
- **Time:** developing dominates, as it already does for HDR (one GPU develop
  per frame). SIFT on 2048 px images and matching take seconds. Compositing is
  CPU and memory-bound.
  - Costly CPU paths that could move to WGSL later: SIFT's Gaussian pyramid
    and the warp.
  - Version 1 stays on the CPU, as HDR and focus merges do, to keep the change
    reviewable.
- **Progress messages:** "Decoding frame i/n", "Developing…", "Finding
  features i/n", "Matching", "Aligning frames", "Balancing exposure",
  "Finding seams", "Blending (strip k/m)", "Saving floating-point TIFF…".

## 8. Order of changes

Each step is one focused change, per AGENTS.md.

1. Move `Plane`/`Texel`/`reduce`/`expand` to `pipeline/pyramid.rs`.
   Extraction: removes future duplication and establishes ownership. Focus
   stack output must be bit-identical.
2. Split `develop_source` out of `merge_sources`, and move `IntermediateMemory`
   to `pipeline/`. No output change.
3. Generalise `alignment::resample` output dimensions. No output change.
4. `panorama/features.rs` + `kdtree.rs` (SIFT port), with the
   `THIRD_PARTY_NOTICES.md` entry for OpenCV's SIFT (Apache-2.0, Rob Hess
   notice).
5. `matching.rs`, `camera.rs` (RANSAC, spanning tree, bundle adjustment, wave
   correction) and a CLI diagnostic, `calibraw-cli panorama-layout <raws…>`,
   that prints the solved rotations, focal length and RMS error. It lets the
   solver be checked on real sets before any compositing exists, which is
   what `calibraw-cli` is for ("diagnostics").
6. `projection.rs`, `exposure.rs`, `seams.rs`, `maxflow.rs`, `crop.rs`.
7. `blend.rs` with the masked reduce, strip compositing and TIFF strip
   writing; `calibraw-gpu::merge_panorama`.
8. UI: `MergeKind::Panorama`, dialog, error kind, locales; DEVELOPMENT.md
   (background work list), ROADMAP, `THIRD_PARTY_NOTICES.md` (OpenCV
   stitching parts, Enblend's masked reduce and halo, Hugin design
   references).
9. Verification on the real-set matrix (§9).

## 9. Verification

- After each change: `cargo test -p calibraw-core`, `cargo test -p calibraw-gpu`.
  At the end, the full AGENTS.md list.
- Steps 1–3: HDR and focus stack TIFFs byte-identical before and after on the
  same inputs.
- In the app, desktop and one Android device, with these sets:
  - 3-frame hand-held landscape row;
  - 8–12-frame 360° row (cylindrical, wrap-around);
  - 2-row × 6 multirow (spherical);
  - portrait-held row;
  - auto-exposure row with sky/ground differences;
  - a lens without a Lensfun profile (the focal estimate path);
  - X-Trans;
  - a set with a walking person in an overlap (the seam goes round them);
  - a set that does not connect (the error names the frame);
  - a set that exceeds the canvas cap (notice, master opens again in
    CalibRaw).
- Each output is opened in Develop and exported once.
- No merge-parity goldens for now (decided 2026-11-20). Regression checks are
  the byte comparisons above, the CLI layout diagnostic and use of the app.

## 10. Later

- **HDR panorama:** HDR-merge each position, then stitch. `PanoramaBuilder`
  takes frames from any source, so this is a UI and grouping feature.
- **Boundary Warp**: a smooth mesh warp that fills the canvas
  rectangle.
- **Fill edges:** an uncropped output whose uncovered corners the existing
  LaMa remove model fills. Until then every panorama is cropped.
- **Merge-parity goldens:** real RAW frames in `data/merge-parity/` with a
  golden each, covering HDR, focus and panorama merges.
- **Vignetting without a profile:** Hugin's polynomial (Vb, Vc, Vd), shared
  by the frames, fitted jointly with exposure on overlap samples.
- **Distortion without a profile:** one shared radial term in the bundle
  adjustment, as Hugin's "b".
- **Parallax:** mesh-based local warps (APAP-style) for close foregrounds.
- **GPU warp and SIFT pyramid.**

## 11. Decisions (2026-11-20)

1. **Auto crop is always on.** There is no switch and no RGBA output in
   version 1.
2. **No merge-parity goldens now.** They are listed in §10.
3. **Desktop and Android in version 1.** On Android the canvas and strip
   height are sized from the RAM allowance (§7). iOS is not planned.

## 12. Implementation notes

Where the implementation differs from the plan above:

- **Module names.** The core stitcher is `calibraw-core/src/pipeline/panorama/`
  with one file per stage. The GPU glue is `calibraw-gpu/src/pipeline/panorama_merge.rs`,
  because a `panorama` module there would shadow the core module re-exported
  through `pipeline::*`. Thumbnail warping got its own `overview.rs`, and the
  frame previews `preview.rs`.
- **Memory allowance.** `IntermediateMemory` stays in the export module, since its
  writer is tied to export outputs. Only the allowance (RAM / 8) is shared, as
  `intermediate_allowance()`. The panorama keeps frames in memory up to half of
  it, stages the rest in memory-mapped files beside the output, and composites in
  the other half.
- **TIFF strips.** The master is written in strips of about 1 MB, as the TIFF
  export does, not one strip per compositing region. libtiff refuses an
  uncompressed final strip shorter than its rows per strip once a strip exceeds
  100 MB (`tif_read.c`).
- **Canvas size on Android.** The output is streamed, so its size does not
  depend on RAM. The canvas is capped only by `MAX_RAW_EDGE` and
  `MAX_RAW_PIXELS`; RAM sets the compositing region size.
- **Exposure prior.** The gain solve uses log gains with a prior of 0.1 times
  each frame's overlap weight, weaker than OpenCV's, since the metadata already
  normalizes the exposures.
- **Seam cost.** Colour only (`COST_COLOR`, OpenCV's `Stitcher` default), not colour
  and gradient: on a real hand-held set the gradient cost halved a near bottle that
  parallax shifted between frames.
- **Focal length.** Read from EXIF `FocalLengthIn35mmFilm` (phones write it), else the
  focal length times Lensfun's crop factor. Without either, pairs are verified by
  homography RANSAC (cpfind's `hom` mode), the focal length is estimated from those
  homographies and each pair's rotation is fitted to its inliers.
- **Black level.** "Too dark to measure" is judged eight stops below each frame's own
  bright level (its 99th-percentile unclipped luminance), not against the sensor's
  white, and the feature image is normalized to that level. An absolute threshold
  masked 75–90 % of dim indoor phone frames, leaving no keypoints.
- **Auto projection.** Perspective up to ±50° at the frame edges, Cylindrical up to
  ±50° of latitude, Spherical beyond.
- **Licences.** OpenCV's stitching sources and `gcgraph.hpp` carry OpenCV's 3-clause
  BSD header, not Apache-2.0. Only the SIFT files are Apache-2.0, with Rob Hess's
  notice. The ported files keep the notices, and `THIRD_PARTY_NOTICES.md`
  reproduces them.
- **CLI.** `calibraw-panorama` runs the whole merge headless and prints its
  diagnostics. It replaces the planned layout-only command.

Second pass (after review of the Sony set showed a split bottle, doubled edges and
steps at the seams). Measured on that set by rendering each frame alone and
phase-correlating 256 px patches: the frames agree to 0–2 px on the back wall, but
differ by 10–42 px on the ceiling and 50–126 px on the floor near the camera. That is
parallax from a hand-held camera turning about the photographer, not the lens's
no-parallax point; no rotation model removes it (PTGui: "correcting for parallax
errors is impossible; the best one can do is attempting to mask the misalignments",
ptgui.com/examples/vptutorial.html). The changes:

- **No clipped-sample seam cost.** The terminal preference for the frame that measured
  a highlight (not in OpenCV) pulled islands of another frame into a region wherever
  highlights were: the bottle's speculars and an LED strip. Seams use OpenCV's terms
  only.
- **Sharp seam mask.** The full-resolution mask is binary, each pixel owned by the
  label with the largest bilinear weight on the seam grid, as Enblend fills its
  scaled-up seam contour at full size (`createMask`, `src/mask.h`). The previous
  bilinear share mixed two misaligned frames over about 11 output pixels, which
  doubled edges.
- **Seam grid.** At 1/8 of the output scale (Enblend's default `--coarse-mask`
  factor) instead of a fixed 0.25 MP, bounded by `MAX_RAW_PIXELS / 64`.
- **Huber threshold 2 px** (Hugin's `SmartOptimise` sets `huberSigma = 2`), instead of
  1.345 robust standard deviations (8 px on that set), so near-object matches stop
  pulling the frames off the distant scene: median residual 4.04 → 2.84 px.
- **Rotation RANSAC tolerance** 25 px per 5000 px of image diagonal (cpfind's default
  `--ransacdist` and its scaling, `RansacMatchesInPairCam`), 42 px on a 7008 × 4672
  frame, instead of 4 registration pixels (16 px). The Sony set without lens
  correction verified only 1 of 3 pairs before.
- **Lens refinement as Hugin's `SmartOptimise`.** For frames no profile corrected,
  one PanoTools radial term `b` is fitted for all frames, discarded beyond |b| > 0.2,
  and the focal length from the metadata is kept unless the frames span 150° (Hugin
  refines the field of view only then). Lens-corrected frames keep refining the
  focal length: CalibRaw enables Lensfun's auto-scale, so the corrected frames no
  longer have the metadata's focal length. On the S25 DNGs: `b` = 0.009, median
  residual 5.21 → 4.83 px, focal length held at the EXIF value (3795 px). On
  synthetic frames rendered with `b` = −0.03, `b` is recovered to ±0.0001.
- **Seam cost without what blending hides.** The colour difference less its local mean
  (two box passes over the shared pixels whose reach matches the blend's coarsest band,
  `2^levels` output pixels), so the cut ignores smooth shading differences the blend
  removes and weighs only edges the frames place apart. On the Sony set the frames shade
  the ceiling differently by 4–7 (0–255 cube-root scale) left of the molding step and by
  under 1 to its right; the plain colour cost therefore led the seam down the right and
  across the molding where parallax had shifted it 12 px. With the local mean removed,
  the steps on the molding, the wall-ceiling edge and the skirting board are gone on the
  Sony and the S25 sets. Cutting on a finer image instead (the registration image,
  max-pooled to the grid) did not help: the cost of crossing a line hardly depends on
  the offset once any is resolved. Photomontage compares gradients for the same reason
  (Agarwala et al., SIGGRAPH 2004).
- **Reference run.** OpenCV 5.1's default `Stitcher` on the same Sony frames (developed
  as JPEG, not lens corrected) shows dark ghosted smudges on the wall and ceiling;
  it has no step where its seam crosses the ceiling molding. Its seam finder, given
  CalibRaw's own warped frames, removes the middle frame and leaves a single seam near
  the room corner, so it never crosses the molding where the frames disagree most.

Verified:

- Real hand-held sets, each three frames of one room:
  - Sony A7C II, 17 mm, lens profile matched: 3/3 pairs, median residual 5.5 px
    over 463 correspondences (parallax, see above), 8128 × 3568 cylindrical master
    in about 63 s; without lens correction 3/3 pairs as well;
  - Galaxy S25 Ultra DNGs, no lens profile, focal length from EXIF: 3/3 pairs,
    median residual 4.83 px, 7608 × 3520;
  - the same phone's JPEGs through the core stitcher, with and without a focal
    length: both converge to the same focal length (2655 px).
- Real RAWs: unrelated frames are refused with an error naming them, and the float
  TIFF reads back with libtiff.
- Synthetic frames rendered with known rotations from a real 7008 × 4672 photo:
  - a 3-frame row and a 2 × 3 grid, with roll, pitch and exposure offsets, in all
    three projections, stitch without visible seams;
  - the focal length is recovered to within 0.1 % and the median residual is
    0.1–0.3 px, with and without focal-length metadata; a rendered radial
    distortion is recovered too.
- HDR merge and focus stacking produce byte-identical TIFFs before and after the
  refactors of steps 1–3.

Pending:

- Android device testing.
- Larger sets (360° rows, multi-row) from a real camera, and a distant scene, where
  parallax is small.
- Parallax itself. Neither Hugin, PTGui nor OpenCV warps frames locally to hide it;
  research methods do (Zaragoza et al., "As-Projective-As-Possible Image Stitching
  with Moving DLT", CVPR 2013; Zhang & Liu, "Parallax-tolerant Image Stitching",
  CVPR 2014). A separate change if wanted.
