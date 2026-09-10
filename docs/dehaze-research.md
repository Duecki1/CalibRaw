# Dehaze mathematics and implementation

## Findings

CalibRaw's previous Dehaze control was dominated by exposure reduction and added saturation. Its inverse-scattering term used `t = 1 − amount × mix(0.008, 0.012, haze_likelihood)`. At maximum strength this removed only 0.8–1.2% of a veil, while separate terms darkened selected tones by up to 0.9 stops and increased perceptual chroma. The result could resemble a contrast or color preset without recovering a meaningful atmospheric transmission signal.

The replacement estimates an atmospheric color shared across the image, computes a dark-channel map, refines that map with a guided filter, and performs inverse scattering before tone edits. Positive values remove estimated haze; negative values add an atmospheric veil. There is no independent dehaze saturation multiplier, local contrast multiplier, or exposure offset.

The implementation targets a useful photographic control, not pixel equality with a proprietary editor. Adobe describes positive Dehaze as removing haze and negative Dehaze as adding simulated haze, including local adjustments. The reviewed Adobe documentation does not specify its estimator, color space, transmission function, or slider calibration. Exact Lightroom mathematics therefore cannot be inferred from that documentation.[^1]

## Research basis and alternatives

The atmospheric model is `I = Jt + A(1 − t)`, with observed RGB `I`, scene RGB `J`, atmospheric RGB `A`, and scalar transmission `t`. It implies `J = A + (I − A)/t`. A local dark-channel prior estimates transmission from the minimum channel of `I/A` within a patch. Its assumption is that clear outdoor patches frequently contain a channel close to black. He, Sun, and Tang also describe estimating airlight from the brightest 0.1% of the dark channel, retaining some haze, and bounding transmission to limit noise amplification.[^2]

A guided filter fits local coefficients `a = cov(g,p)/(var(g)+epsilon)` and `b = mean(p) − a mean(g)`, then reconstructs `q = mean(a)g + mean(b)`. Both averaging operations matter: retaining only the first regression would not implement the filter described by He, Sun, and Tang. The guidance image steers the result at edges instead of simply blurring the transmission map.[^3]

These methods are relevant to photographic software: darktable's haze-removal source explicitly identifies dark-channel estimation and guided filtering, declares linear scene-referred RGB input/output, and implements `(input − A)/t + A` with bounded transmission. This provides an independently inspectable editor implementation of the same physical foundation. Its parameters and implementation details are not treated as Lightroom specifications.[^4]

Other published approaches address different weaknesses. Fattal estimates transmission from local color-line distributions, rejects patches inconsistent with the model, and uses a field model to propagate estimates.[^5] Berman, Treibitz, and Avidan use non-local color clusters stretched into haze-lines; their method is deterministic and does not require training.[^6]

| Approach | Relevant evidence | Engineering assessment for this change |
| --- | --- | --- |
| Previous darkening/chroma heuristic | Existing CalibRaw shader | Cannot meaningfully invert a thick veil with transmission near one. |
| Dark channel plus guided filter | Published estimator and filter; inspectable darktable implementation | Selected: explicit equations, bounded local support, and practical integration with tiled GPU processing. |
| Local color-lines | Transmission estimates include hypothesis validation | A possible later alternative for scenes that violate the dark-channel assumption; requires an additional estimator and propagation system. |
| Non-local haze-lines | Uses relationships between pixels across the image | A possible later alternative; would need global clustering and coordination with crop/export processing. |

The choice is an integration judgment, not a claim that dark-channel dehazing is the newest or universally most accurate method. It directly addresses the defects found in this code while keeping the result testable against a known image-formation model.

## Defects in the previous implementation

The previous code mixed several domains and objectives. It sampled already adjusted scene values but derived atmospheric brightness from a pre-adjustment luminance percentile. It selected an atmospheric color separately within each sampled neighborhood and blended that color strongly toward neutral. The same spatially varying object colors could therefore influence both the haze estimate and the color of the light being subtracted.

The neighborhood used spaced samples rather than a complete minimum filter. A small dark object between samples could be missed, while changes in the sampling scale could alter the estimate. There was no explicit edge-refined transmission map. Instead, a bilateral luminance statistic controlled a small physical term and additional local contrast.

The final appearance then depended on three independent additions: an exposure-shaped darkening term, a local-detail multiplier, and an Oklab chroma increase. Those additions could not be validated by checking recovery of known atmospheric scattering. They also made it difficult to distinguish intended haze removal from unrelated color changes.

## Implemented processing model

The following constants and safeguards are CalibRaw design choices. They are not presented as published Lightroom settings.

### Atmospheric color

Atmospheric analysis runs with the existing tone-analysis stage on characterized, unexposed, linear Rec.2020 RGB. Each complete cell of the globally aligned tone grid contributes its minimum channel value and mean RGB. Cells are four pixels wide on desktop and eight on Android, matching the existing tone guide.

The cell minimum chooses a logarithmic histogram bin. Each bin accumulates a candidate count and normalized RGB sums. Reduction walks from the brightest dark-channel bin downward until it reaches at least 0.1% of candidates, then averages those candidates. The entire threshold bin is included to make ties independent of submission order. Unlike selecting one brightest input pixel, averaging a dark-selected cell population reduces dependence on an individual sample.

This is a deliberate approximation to the paper's full-image dark-channel selection. The atmosphere-estimation cell is smaller than the transmission-estimation patch. It rejects isolated bright pixels inside an otherwise dark cell, but a sufficiently large white object can still bias the estimate. It does not infer illumination semantics.

RGB sums use fixed-point contributions with an explicit carry word, avoiding wraparound during large-image accumulation. Values are normalized to each logarithmic bin; the bounded per-contribution representation is intended to preserve bright, nearly neutral candidates. Strongly chromatic candidates with channel ratios above the representation's 64-fold limit are approximated.

The resulting RGB and candidate count extend the shared tone statistics from 32 to 48 bytes. All export tiles and inherited detail crops receive the same statistics. Contributions are restricted to histogram core bounds, excluding duplicated tile halos. Incomplete grid cells are omitted; an image too small to provide a complete cell bypasses positive restoration instead of trusting a nonexistent estimate.

### Dark channel and spatial scale

At output time the atmospheric RGB is multiplied by the current global exposure gain. The prepared input receives the same gain, so the ratios used for estimation are exposure-invariant, apart from numerical floors and finite precision.

The shader first computes a horizontal minimum of `min(R/A_R, G/A_G, B/A_B)`, then a vertical minimum. These passes cover every pixel in the square patch. The radius follows the existing image-size scaling convention: seven reference pixels, rounded and clamped to 1–14 processing pixels. It uses full-image dimensions when processing a tile, not tile dimensions.

The maximum radius is a performance and tile-support limit. Consequently the relative patch size is not perfectly invariant between a small preview and a very large export. The precision comparison test covers equal-resolution preview and export formats; it does not prove equality across arbitrary resampling ratios.

### Guided refinement

The guide is bounded relative luminance: `g = y/(1+y)`, where `y = luminance(I)/luminance(A)`. This avoids using an unbounded HDR value directly in the local regression. The implementation computes means of `g`, the dark map, `g²`, and their product, derives the guided coefficients, and averages those coefficients during reconstruction.

The reference guide radius is four pixels and its maximum is eight. Regularization is `0.0001` in this bounded guide domain. Estimation uses a scalar luminance guide; equal-luminance color boundaries therefore do not receive the full discrimination of a three-channel guided filter. This is a known tradeoff in scratch storage and shader cost.

The dark map is clamped after refinement. Close to completely opaque, airlight-like content, a smooth confidence factor reduces the inferred veil: `confidence = 1 − smoothstep(0.95, 1, dark)`. This narrowly targeted safeguard protects ambiguous nearly uniform regions; it can also leave residual haze where real transmission is extremely low.

### Slider mapping and recovery

For positive strength `s = clamp(value/100, 0, 1)`, the estimated transmission is `t = max(1 − 0.95 × dark × confidence, 0.15)`. The requested removal is `v = 1 − t^s`. Thus the slider scales inferred optical depth and reaches the bounded estimate at +100. The relationship is smooth and has an identity endpoint at zero.

Before subtraction, `v` is limited to 98% of the minimum available channel ratio of the current pixel. The output is `(I − A×v)/(1−v)`. This gives all channels one transmission and bounds the subtraction by available signal, reducing negative channels at colored objects and depth edges. The 0.15 transmission floor limits restoration gain to about 6.67. Saturation changes arise from removal of atmospheric light, without a separate color boost.

For negative strength, `t_add = min(t, 0.65)^abs(s)` and `output = A + (I − A)×t_add`. The 0.65 cap gives negative Dehaze a visible effect even in a clear scene whose estimated transmission is near one. This is a creative forward-scattering control, not an assertion that clear pixels have measured haze.

Zero strength bypasses all five image passes when no enabled adjustment mask requests Dehaze. Within an active pass, zero returns the input unchanged.

## Pipeline integration

The processing order is now:

1. Camera characterization and global exposure.
2. Atmospheric restoration or haze addition, including local Dehaze masks.
3. Local exposure, capture sharpening, global tone adjustments, and local tone adjustments.
4. Texture, clarity, saturation/vibrance, remaining effects, and display conversion.

Local exposure is applied after Dehaze so both the estimated atmosphere and image samples remain in the same domain during restoration. When Dehaze is inactive, local exposure stays in the original preparation pass. This preserves the existing input to neighborhood-based capture sharpening. With Dehaze active it is applied by the final copy pass, before sharpening reads the prepared image.

Local Dehaze uses the common atmosphere and transmission estimate, then blends each mask's adjusted result by its coverage. Global and overlapping local adjustments are processed sequentially. A second positive adjustment is still constrained by the remaining channel signal. Mask edges do not redefine the atmosphere or truncate the estimator's input region.

The GPU work consists of horizontal minimum, vertical minimum, guided coefficients, restoration, and a copy that finishes scene preparation. Existing highlight scratch textures are reused after raw processing has finished; no new full-resolution textures are allocated. The histogram grows by 7 KiB and the statistics buffer by 16 bytes before alignment. Programs remain lazily compiled and compatible with the existing program-template reuse scheme.

Spatial support is cumulative. The dark minimum reaches 14 pixels and guided regression/reconstruction adds two eight-pixel windows, so Dehaze requires 30 pixels before subsequent local-detail support. Export halo calculation includes that additional support for both global and enabled local Dehaze.

Atmospheric analysis still executes during tone analysis even with a neutral Dehaze setting, making later slider edits possible without rerunning tone analysis. This adds fixed histogram work per analysis. The two guided-filter windows currently use direct square sums; they are bounded but cost more texture reads at larger radii. Mobile-device performance has not been benchmarked.

## Validation

The regression fixture starts with known clear RGB, mixes it with colored atmospheric RGB `[0.8, 0.9, 1.0]` at transmission 0.5, and executes the production GPU estimation and restoration passes. Reconstruction is measured before unrelated tone rendering.

| Dehaze | Scene-linear RGB root-mean-square error |
| ---: | ---: |
| 0 | 0.331757 |
| +25 | 0.273782 |
| +50 | 0.205658 |
| +100 | 0.031631 |

Full strength reduces this fixture's error by about 90.5%. This is a regression against a known synthetic scattering process, not a benchmark against Lightroom or a claim of universal photographic accuracy.

Additional executable checks cover:

- Clear textured content, dense haze at transmission 0.2, black and grey flat fields, HDR values above one, and a one-pixel image.
- Positive-strength progression and finite, nonnegative output for nonnegative test input.
- Covariance with a one-stop global exposure change.
- Negative Dehaze moving RGB toward the atmospheric color.
- Local-only activation, disabled-mask bypass, and exact preservation outside mask coverage.
- Detail-crop agreement with the full-frame render and inherited atmosphere.
- Accumulation of disjoint export histogram cores in a different order.
- Equal-resolution preview/export precision agreement within three 8-bit output code values.
- WGSL composition/validation for both working formats and the existing GPU pipeline regressions.

Published forest and Tiananmen inputs from the original dehazing project were also rendered through the headless TIFF export path and inspected at +70.[^7] The architectural example shows increased visibility and color separation; the forest retains substantial deep fog, and noise is more visible at strong settings. These are qualitative checks using processed source photographs without scene-linear ground truth. They do not establish a numerical ranking against the authors' outputs. Third-party images are not included in the repository.

Final validation on the Linux Vulkan test environment: 63 GPU-crate tests and
18 core processing tests passed; `cargo check --workspace --all-targets` passed.
Strict Clippy checks of the core and GPU libraries also passed. The broader
all-target Clippy check reports existing `field_reassign_with_default` violations
in older tests, and the workspace-wide format check reports existing formatting
differences. Those unrelated files/sections were not reformatted as part of this
change. No Windows or Android runtime was exercised.

Reproduce the automated checks with:

```sh
cargo test -p calibraw-gpu --lib -- --nocapture
cargo test -p calibraw-core --lib pipeline::processing::tests
cargo check --workspace --all-targets
```

For a photographic comparison, use a supported RAW or raster TIFF and keep every other adjustment fixed:

```sh
cargo run -p calibraw-cli --bin calibraw-develop-export -- \
  --input photo.tiff --output dehaze-70.png --adjust dehaze=70
```

## Limits and compatibility

A single photograph cannot uniquely identify atmospheric light, transmission, and original reflectance. Dark-channel methods can misinterpret bright uniform objects lacking dark samples, and the original work explicitly acknowledges that limitation.[^2] The safeguards here reduce specific failure modes; they do not solve the ambiguity.

Residual fog in nearly opaque regions, amplified sensor/compression noise, and imperfect color recovery under spatially varying illumination remain possible. Separate channel-dependent attenuation, such as underwater imagery, is outside this scalar-transmission model. No learned detail synthesis or texture reconstruction is performed.

The control retains its existing name, range, default, and sidecar field. Existing nonzero Dehaze edits render differently because the processing behavior is corrected. Zero Dehaze retains the previous processing path except for collection of unused atmospheric statistics. No sidecar migration or attempt to preserve the old stylized appearance is introduced.

There is no direct Lightroom comparison set supplied with this change. Matching Adobe's exact strength curve would require controlled paired renders across diverse scenes and versions, and even then would be an empirical calibration rather than recovery of proprietary source mathematics. The delivered implementation is independently specified and tested.

## Sources

1. Adobe. *Photoshop Lightroom reference*, October 15, 2018, Dehaze description; *Adjust effects*, December 3, 2024. [Lightroom reference](https://helpx.adobe.com/pdf/lightroom_reference.pdf), [current web effects documentation](https://helpx.adobe.com/ca/lightroom-cc/web/edit-photos/apply-effects/adjust-effects.html).
2. Kaiming He, Jian Sun, and Xiaoou Tang. *Single Image Haze Removal Using Dark Channel Prior*. CVPR 2009, sections 2–4. [Author-hosted paper](https://people.csail.mit.edu/kaiming/publications/cvpr09.pdf).
3. Kaiming He, Jian Sun, and Xiaoou Tang. *Guided Image Filtering*. ECCV 2010, section 3.1. [Author-hosted paper](https://people.csail.mit.edu/kaiming/publications/eccv10guidedfilter.pdf).
4. darktable contributors. *src/iop/hazeremoval.c*, development source inspected September 10, 2026. [Source](https://raw.githubusercontent.com/darktable-org/darktable/master/src/iop/hazeremoval.c). This is a moving development reference, not a pinned release behavior claim.
5. Raanan Fattal. *Dehazing using color-lines*. ACM Transactions on Graphics 34(1), article 13, published December 29, 2014. [University publication record](https://cris.huji.ac.il/en/publications/dehazing-using-color-lines-2/), [DOI](https://doi.org/10.1145/2651362).
6. Dana Berman, Tali Treibitz, and Shai Avidan. *Non-Local Image Dehazing*. CVPR 2016, pp. 1674–1682. [CVF publication record](https://openaccess.thecvf.com/content_cvpr_2016/html/Berman_Non-Local_Image_Dehazing_CVPR_2016_paper.html).
7. Kaiming He. *Single Image Haze Removal*, project demonstrations. [Project page](https://people.csail.mit.edu/kaiming/cvpr09/index.html), [forest input](https://people.csail.mit.edu/kaiming/cvpr09/forest/forest1.jpg), [Tiananmen input](https://people.csail.mit.edu/kaiming/cvpr09/tiananmen/tiananmen1.png).

[^1]: Adobe, [Lightroom reference](https://helpx.adobe.com/pdf/lightroom_reference.pdf), 2018, Dehaze; [Adjust effects](https://helpx.adobe.com/ca/lightroom-cc/web/edit-photos/apply-effects/adjust-effects.html), 2024.
[^2]: He, Sun, and Tang, [Single Image Haze Removal Using Dark Channel Prior](https://people.csail.mit.edu/kaiming/publications/cvpr09.pdf), CVPR 2009.
[^3]: He, Sun, and Tang, [Guided Image Filtering](https://people.csail.mit.edu/kaiming/publications/eccv10guidedfilter.pdf), ECCV 2010, equations 5–8.
[^4]: darktable contributors, [hazeremoval.c](https://raw.githubusercontent.com/darktable-org/darktable/master/src/iop/hazeremoval.c), inspected September 10, 2026.
[^5]: Fattal, [Dehazing using color-lines](https://cris.huji.ac.il/en/publications/dehazing-using-color-lines-2/), 2014.
[^6]: Berman, Treibitz, and Avidan, [Non-Local Image Dehazing](https://openaccess.thecvf.com/content_cvpr_2016/html/Berman_Non-Local_Image_Dehazing_CVPR_2016_paper.html), 2016.
[^7]: He, [Single Image Haze Removal project demonstrations](https://people.csail.mit.edu/kaiming/cvpr09/index.html).
