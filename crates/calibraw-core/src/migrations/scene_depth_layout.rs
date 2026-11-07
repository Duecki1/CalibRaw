//! Scene depth saved before schema 5: one 8-bit map, with or without the
//! normals predicted with it.
//!
//! migration: remove in v2.0.0
//!
//! Schemas 1 to 4 stored depth as an 8-bit map `b` at the size of the image
//! the depth job saw (up to 4096 pixels on the longer side): 0 nearest, 255
//! for the sky and for what lay beyond the depth model's far limit. Up to v1.1
//! the map was inline in the edit (`edits.masks.scene_depth`) and in each
//! Depth Range component; later it was a mask asset (`scene_depth_asset`)
//! that Depth Range components referenced too, and development builds after
//! v1.4 added the normals' x and y as assets beside it
//! (`scene_normal_assets`). [`LegacySceneDepth`] reads those fields from the
//! same document, and [`LegacySceneDepth::take_scene_depth`] turns them into a
//! `SceneDepth` that renders as those versions did:
//!
//! 1. Disparity `i = 1 − b/255` at the map's own size, smoothed within each
//!    8-bit level so that slopes lose their terraces. Smoothing never crosses
//!    a step of more than [`SMOOTH_STEP`] levels or the sky, and stays within
//!    half a level of every sample, so `b` is exactly what rounding returns.
//! 2. Sky probability 1 where `b` is 255, else 0, at the same size: what
//!    those versions treated as sky.
//! 3. The camera those versions assumed: `D(i) = 1 / (i + 0.115)`, anchors at
//!    disparity 1 and 0, a tangent of 0.4 and the frame of Relight's default
//!    Relief, so linear depth and the frame are exactly theirs.
//! 4. The stored normals, or, without them (v1.4 and older), normals of the
//!    smoothed disparity in that frame.
//!
//! Depth Range masks select on exactly the old map, as the selection map
//! rounds back to `b`.

use crate::pipeline::{
    DisparityMap, MaskGeometry, MaskImage, MaskStack, SceneCamera, SceneDepth, SceneNormals,
};
use crate::sidecar::{
    decode_mask_png, invalid, validate_image, SidecarError, SidecarMaskAsset, SidecarMaskAssetRef,
    MAX_DECODED_ASSET_BYTES,
};
use rayon::prelude::*;
use serde::de::IgnoredAny;
use serde::Deserialize;

/// The stored depth of the sky and of what lay beyond the model's far limit.
const SKY: u8 = 255;
/// Distance in those versions: `D(i) = 1 / (i + LEGACY_SHIFT)`.
const LEGACY_SHIFT: f32 = 0.115;
/// Tangent of half the field of view across the shorter edge they assumed.
const LEGACY_FOCAL_TAN: f32 = 0.4;
/// Depth range of their frame: Relight's default Relief 50, which every other
/// effect used, as `0.25 + 5.75 · 0.5^1.5`.
const LEGACY_FRAME_RELIEF: f32 = 2.282_932;
/// The camera of a migrated result.
const LEGACY_CAMERA: SceneCamera = SceneCamera {
    focal_tan: LEGACY_FOCAL_TAN,
    fit: [1.0, LEGACY_SHIFT],
    anchors: [1.0, 0.0],
    frame_relief: LEGACY_FRAME_RELIEF,
};

/// Largest step, in map levels between neighbouring pixels, that smoothing
/// and normals treat as one surface: one quantization step and some noise.
const SMOOTH_STEP: u8 = 2;
/// Smoothing width as a share of the map's longer edge: wider than the
/// terraces of an 8-bit slope.
const SMOOTH_SHARE: f32 = 1.0 / 128.0;
/// Iterations of the recursive smoothing filter.
const SMOOTH_PASSES: i32 = 3;
/// Longest edge of derived normals, about the normal model's.
const NORMAL_EDGE: usize = 1540;
/// Decoded bytes of a result per map pixel: disparity (2), sky and selection
/// map (1 each).
const DECODED_BYTES_PER_PIXEL: u64 = 4;

/// Where schemas 1 to 4 kept scene depth, read from the same document as the
/// rest of the edit. Every other field is ignored.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct LegacySceneDepth {
    #[serde(default)]
    scene_depth_asset: Option<usize>,
    /// The normals' x and y.
    #[serde(default)]
    scene_normal_assets: Option<[usize; 2]>,
    #[serde(default)]
    edits: LegacyEdits,
}

#[derive(Debug, Default, Deserialize)]
struct LegacyEdits {
    #[serde(default)]
    masks: LegacyMasks,
}

#[derive(Debug, Default, Deserialize)]
struct LegacyMasks {
    /// Inline pixels, up to v1.1.
    #[serde(default)]
    scene_depth: Option<MaskImage>,
    /// In the same order as the decoded edit's masks.
    #[serde(default)]
    masks: Vec<LegacyMask>,
}

#[derive(Debug, Default, Deserialize)]
struct LegacyMask {
    #[serde(default)]
    components: Vec<LegacyComponent>,
}

#[derive(Debug, Default, Deserialize)]
struct LegacyComponent {
    #[serde(default)]
    geometry: LegacyGeometry,
}

/// A component's geometry: a Depth Range one, or any other.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum LegacyGeometry {
    DepthRange {
        #[serde(rename = "DepthRange")]
        depth_range: LegacyDepthRange,
    },
    Other(IgnoredAny),
}

impl Default for LegacyGeometry {
    fn default() -> Self {
        Self::Other(IgnoredAny)
    }
}

#[derive(Debug, Deserialize)]
struct LegacyDepthRange {
    /// Inline pixels, up to v1.1.
    #[serde(default)]
    depth: Option<MaskImage>,
}

impl LegacySceneDepth {
    /// The scene depth of the document, taken out of its mask `assets`: the
    /// depth and normal assets and the Depth Range components' `references`
    /// are removed, and the remaining references renumbered. `masks` is the
    /// document's decoded mask stack, before any asset is restored.
    pub(crate) fn take_scene_depth(
        self,
        assets: &mut Vec<SidecarMaskAsset>,
        references: &mut Vec<SidecarMaskAssetRef>,
        masks: &MaskStack,
    ) -> Result<Option<SceneDepth>, SidecarError> {
        let Self {
            scene_depth_asset,
            scene_normal_assets,
            edits: LegacyEdits { masks: legacy },
        } = self;
        if scene_depth_asset.is_some() && legacy.scene_depth.is_some() {
            return invalid("scene depth has both inline pixels and an asset reference");
        }
        if scene_normal_assets.is_some() && scene_depth_asset.is_none() {
            return invalid("scene normals are stored without scene depth");
        }
        let targets_depth_range = |reference: &SidecarMaskAssetRef| {
            masks
                .masks
                .get(reference.mask_index)
                .and_then(|mask| mask.components.get(reference.component_index))
                .is_some_and(|component| {
                    matches!(component.geometry, MaskGeometry::DepthRange { .. })
                })
        };
        let (depth_range_refs, kept_refs): (Vec<&SidecarMaskAssetRef>, Vec<_>) = references
            .iter()
            .partition(|reference| targets_depth_range(reference));
        let inline_depth_ranges: Vec<(usize, usize, MaskImage)> = legacy
            .masks
            .into_iter()
            .enumerate()
            .flat_map(|(mask_index, mask)| {
                mask.components.into_iter().enumerate().filter_map(
                    move |(component_index, component)| match component.geometry {
                        LegacyGeometry::DepthRange { depth_range } => depth_range
                            .depth
                            .map(|depth| (mask_index, component_index, depth)),
                        LegacyGeometry::Other(_) => None,
                    },
                )
            })
            .collect();
        if inline_depth_ranges.iter().any(|(mask, component, _)| {
            depth_range_refs.iter().any(|reference| {
                reference.mask_index == *mask && reference.component_index == *component
            })
        }) {
            return invalid("generated mask has both inline pixels and an asset reference");
        }

        let asset = |index: usize| {
            assets.get(index).ok_or_else(|| {
                SidecarError::Invalid(
                    "scene depth reference uses an invalid asset index".to_owned(),
                )
            })
        };
        // The stack's own map; without one, the first Depth Range
        // component's, which those versions' effects fell back to.
        let map = match (scene_depth_asset, legacy.scene_depth) {
            (Some(index), _) => Some(decode_mask_png(asset(index)?)?),
            (None, Some(image)) => Some(image),
            (None, None) => match depth_range_refs.first() {
                Some(reference) => Some(decode_mask_png(asset(reference.asset_index)?)?),
                None => inline_depth_ranges
                    .into_iter()
                    .next()
                    .map(|(_, _, image)| image),
            },
        };
        let normals = scene_normal_assets
            .map(|[x, y]| {
                SceneNormals::new(decode_mask_png(asset(x)?)?, decode_mask_png(asset(y)?)?)
                    .ok_or_else(|| {
                        SidecarError::Invalid("scene normal components differ in size".to_owned())
                    })
            })
            .transpose()?;

        let removed: Vec<usize> = scene_depth_asset
            .into_iter()
            .chain(scene_normal_assets.into_iter().flatten())
            .chain(
                depth_range_refs
                    .iter()
                    .map(|reference| reference.asset_index),
            )
            .collect();
        let kept_refs: Vec<SidecarMaskAssetRef> = kept_refs.into_iter().copied().collect();
        remove_assets(assets, &removed, &kept_refs, references);

        let Some(map) = map else {
            return Ok(None);
        };
        validate_image(map.width, map.height, map.pixels.len(), 1)?;
        let decoded = (map.pixels.len() as u64)
            .checked_mul(DECODED_BYTES_PER_PIXEL)
            .ok_or(SidecarError::TooLarge(u64::MAX))?;
        if decoded > MAX_DECODED_ASSET_BYTES {
            return invalid("scene depth exceeds the decoded memory safety limit");
        }
        Ok(Some(scene_depth(&map, normals)))
    }
}

/// Removes the `removed` assets that no reference in `kept` still uses, and
/// sets `references` to `kept`, renumbered. An index beyond the table stays
/// beyond it, so restoring rejects it.
fn remove_assets(
    assets: &mut Vec<SidecarMaskAsset>,
    removed: &[usize],
    kept: &[SidecarMaskAssetRef],
    references: &mut Vec<SidecarMaskAssetRef>,
) {
    let mut keep = vec![true; assets.len()];
    for &index in removed {
        if let Some(flag) = keep.get_mut(index) {
            *flag = false;
        }
    }
    for reference in kept {
        if let Some(flag) = keep.get_mut(reference.asset_index) {
            *flag = true;
        }
    }
    let mut renumbered = Vec::with_capacity(assets.len());
    let mut next = 0;
    for &kept in &keep {
        renumbered.push(next);
        next += usize::from(kept);
    }
    let mut index = 0;
    assets.retain(|_| {
        index += 1;
        keep[index - 1]
    });
    *references = kept
        .iter()
        .map(|reference| SidecarMaskAssetRef {
            asset_index: renumbered
                .get(reference.asset_index)
                .copied()
                .unwrap_or(reference.asset_index),
            ..*reference
        })
        .collect();
}

/// The result an 8-bit `map` describes, with the `normals` stored beside it
/// (module docs).
fn scene_depth(map: &MaskImage, normals: Option<SceneNormals>) -> SceneDepth {
    let disparity = dequantized_disparity(map);
    let normals = normals.unwrap_or_else(|| derived_normals(map, &disparity, &LEGACY_CAMERA));
    let sky = map
        .pixels
        .iter()
        .map(|&stored| if stored == SKY { 255 } else { 0 })
        .collect();
    let sky =
        MaskImage::new(map.width, map.height, sky).expect("the sky has one pixel per map pixel");
    SceneDepth::new(disparity, sky, normals, LEGACY_CAMERA)
        .expect("the legacy camera is valid and the sky is not empty")
}

/// Whether smoothing and normals treat neighbouring map values as one surface.
fn continuous(a: u8, b: u8) -> bool {
    a != SKY && b != SKY && a.abs_diff(b) <= SMOOTH_STEP
}

/// Disparity of `map` (module docs, step 1): a recursive edge-stopping filter
/// (the domain transform's, Gastal and Oliveira 2011, with a binary edge test),
/// applied to rows and columns in turn, then held within half a level.
fn dequantized_disparity(map: &MaskImage) -> DisparityMap {
    let width = map.width as usize;
    let height = map.height as usize;
    // Disparity in map levels: 255 nearest, 0 farthest.
    let levels: Vec<f32> = map
        .pixels
        .iter()
        .map(|&stored| f32::from(SKY - stored))
        .collect();
    let mut smooth = levels.clone();
    let sigma = (width.max(height) as f32 * SMOOTH_SHARE).max(1.0);
    for pass in 0..SMOOTH_PASSES {
        // Iteration widths that together average like one of `sigma`.
        let sigma_pass = sigma * 3.0_f32.sqrt() * 2.0_f32.powi(SMOOTH_PASSES - 1 - pass)
            / (4.0_f32.powi(SMOOTH_PASSES) - 1.0).sqrt();
        let feedback = (-std::f32::consts::SQRT_2 / sigma_pass).exp();
        smooth
            .par_chunks_mut(width)
            .zip(map.pixels.par_chunks(width))
            .for_each(|(row, stored)| {
                for x in 1..width {
                    if continuous(stored[x - 1], stored[x]) {
                        row[x] += feedback * (row[x - 1] - row[x]);
                    }
                }
                for x in (0..width.saturating_sub(1)).rev() {
                    if continuous(stored[x + 1], stored[x]) {
                        row[x] += feedback * (row[x + 1] - row[x]);
                    }
                }
            });
        let column_step = |smooth: &mut [f32], from: usize, to: usize| {
            for x in 0..width {
                if continuous(map.pixels[from + x], map.pixels[to + x]) {
                    smooth[to + x] += feedback * (smooth[from + x] - smooth[to + x]);
                }
            }
        };
        for y in 1..height {
            column_step(&mut smooth, (y - 1) * width, y * width);
        }
        for y in (0..height.saturating_sub(1)).rev() {
            column_step(&mut smooth, (y + 1) * width, y * width);
        }
    }
    // A level L is the stored values 257 L ± 128.5 (65535 = 255 · 257), so
    // holding the value within 128 of 257 L rounds back to L.
    let values = levels
        .iter()
        .zip(&smooth)
        .zip(map.pixels.iter())
        .map(|((&level, &smooth), &stored)| {
            if stored == SKY {
                return 0;
            }
            let centre = level * 257.0;
            (smooth * 257.0)
                .round()
                .clamp(centre - 128.0, centre + 128.0)
                .min(f32::from(DisparityMap::NEAREST)) as u16
        })
        .collect();
    DisparityMap::new(map.width, map.height, values)
        .expect("the disparity has one value per map pixel")
}

/// Normals of the surface `disparity` describes in `camera`'s frame, at most
/// [`NORMAL_EDGE`] on the longer side (module docs, step 4). Each comes from
/// the positions a sample step away on either side, one side only across a
/// step of the map or the sky, and facing the camera along an axis where
/// neither side continues the surface. Sky faces the camera.
fn derived_normals(
    map: &MaskImage,
    disparity: &DisparityMap,
    camera: &SceneCamera,
) -> SceneNormals {
    let width = map.width as usize;
    let height = map.height as usize;
    let stride = width.max(height).div_ceil(NORMAL_EDGE).max(1);
    let out_width = width.div_ceil(stride);
    let out_height = height.div_ceil(stride);
    let shorter = width.min(height) as f32;
    let centre = [
        width as f32 / (2.0 * shorter),
        height as f32 / (2.0 * shorter),
    ];
    let stored = |x: usize, y: usize| map.pixels[y * width + x];
    let position = |x: usize, y: usize| {
        let z = camera.frame_depth(DisparityMap::decode(disparity.values()[y * width + x]));
        let lateral = 2.0 * camera.focal_tan * z;
        [
            ((x as f32 + 0.5) / shorter - centre[0]) * lateral,
            ((y as f32 + 0.5) / shorter - centre[1]) * lateral,
            z,
        ]
    };
    // Whether the map runs from `here` to `there`, `count` pixels along
    // (dx, dy), without sky or a step.
    let runs_to = |here: (usize, usize), step: (isize, isize), count: usize| {
        let (mut x, mut y) = here;
        for _ in 0..count {
            let next_x = x.wrapping_add_signed(step.0);
            let next_y = y.wrapping_add_signed(step.1);
            if !continuous(stored(x, y), stored(next_x, next_y)) {
                return false;
            }
            (x, y) = (next_x, next_y);
        }
        count > 0
    };
    // The change of position per pixel along one axis at (x, y).
    let tangent = |x: usize, y: usize, horizontal: bool| -> [f32; 3] {
        let (along, extent) = if horizontal { (x, width) } else { (y, height) };
        let back = along.min(stride);
        let ahead = (extent - 1 - along).min(stride);
        let step = |sign: isize| if horizontal { (sign, 0) } else { (0, sign) };
        let point = |offset: isize| {
            if horizontal {
                position(x.wrapping_add_signed(offset), y)
            } else {
                position(x, y.wrapping_add_signed(offset))
            }
        };
        let (from, to) = match (
            runs_to((x, y), step(-1), back),
            runs_to((x, y), step(1), ahead),
        ) {
            (true, true) => (-(back as isize), ahead as isize),
            (true, false) => (-(back as isize), 0),
            (false, true) => (0, ahead as isize),
            (false, false) => {
                let lateral = 2.0 * camera.focal_tan * position(x, y)[2] / shorter;
                return if horizontal {
                    [lateral, 0.0, 0.0]
                } else {
                    [0.0, lateral, 0.0]
                };
            }
        };
        let [a, b] = [point(from), point(to)];
        let span = (to - from) as f32;
        [
            (b[0] - a[0]) / span,
            (b[1] - a[1]) / span,
            (b[2] - a[2]) / span,
        ]
    };
    let mut x_components = vec![SceneNormals::encode(0.0); out_width * out_height];
    let mut y_components = x_components.clone();
    x_components
        .par_chunks_mut(out_width)
        .zip(y_components.par_chunks_mut(out_width))
        .enumerate()
        .for_each(|(out_y, (row_x, row_y))| {
            let y = (out_y * stride + stride / 2).min(height - 1);
            for out_x in 0..out_width {
                let x = (out_x * stride + stride / 2).min(width - 1);
                if stored(x, y) == SKY {
                    continue;
                }
                let along_x = tangent(x, y, true);
                let along_y = tangent(x, y, false);
                // along_y × along_x faces the camera (negative z) in a frame
                // with x right, y down and z away.
                let normal = [
                    along_y[1] * along_x[2] - along_y[2] * along_x[1],
                    along_y[2] * along_x[0] - along_y[0] * along_x[2],
                    along_y[0] * along_x[1] - along_y[1] * along_x[0],
                ];
                let length = normal.iter().map(|value| value * value).sum::<f32>().sqrt();
                if length.is_finite() && length > 0.0 {
                    row_x[out_x] = SceneNormals::encode(normal[0] / length);
                    row_y[out_x] = SceneNormals::encode(normal[1] / length);
                }
            }
        });
    let component = |pixels| {
        MaskImage::new(out_width as u32, out_height as u32, pixels)
            .expect("each normal component has one pixel per sample")
    };
    SceneNormals::new(component(x_components), component(y_components))
        .expect("both normal components have the same size")
}
