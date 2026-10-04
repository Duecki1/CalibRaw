use super::{color_profile::display_linear_rec2020_to_srgb, ExposureParams, LoadedRaw};
use crate::color_math::{linear_srgb_to_rec2020, srgb_decode};
use crate::matrix;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::sync::{Arc, OnceLock};

mod brush;
mod composite;
mod passes;
mod scene_color;
pub use brush::*;
pub use composite::*;
pub use passes::*;
pub use scene_color::*;

pub const BIG_LAMA_INPUT_EDGE: u32 = 512;
pub const REMOVE_MAX_STROKES: usize = 512;
pub const REMOVE_MAX_POINTS_PER_STROKE: usize = 65_536;
pub const REMOVE_MAX_PATCHES_PER_STROKE: usize = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetouchTool {
    #[default]
    Clone,
    Heal,
}

impl RetouchTool {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Clone => "Clone",
            Self::Heal => "Heal",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetouchAlignment {
    #[default]
    None,
    Aligned,
    Registered,
    Fixed,
}

impl RetouchAlignment {
    pub const ALL: [Self; 4] = [Self::None, Self::Aligned, Self::Registered, Self::Fixed];

    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Aligned => "Aligned",
            Self::Registered => "Registered",
            Self::Fixed => "Fixed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetouchStroke {
    pub tool: RetouchTool,
    pub alignment: RetouchAlignment,
    pub source: [f32; 2],
    pub destination: [f32; 2],
    /// GIMP-style hard-center fraction. The remaining radius is feathered.
    pub hardness: f32,
    pub opacity: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RemoveBrushPoint {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RemoveBrushStroke {
    pub points: Vec<RemoveBrushPoint>,
    #[serde(default)]
    pub dilation_radius: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl NativeRect {
    pub fn right(self) -> u32 {
        self.x.saturating_add(self.width)
    }

    pub fn bottom(self) -> u32 {
        self.y.saturating_add(self.height)
    }

    pub fn intersect(self, other: Self) -> Option<Self> {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = self.right().min(other.right());
        let y1 = self.bottom().min(other.bottom());
        // Lazy: the sizes underflow when the rectangles do not overlap.
        (x1 > x0 && y1 > y0).then(|| Self {
            x: x0,
            y: y0,
            width: x1 - x0,
            height: y1 - y0,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemovePatchSidecarCache {
    pub fingerprint: u64,
    pub rgb_png: Arc<[u8]>,
    pub alpha_png: Arc<[u8]>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemovePatch {
    pub bounds: NativeRect,
    #[serde(
        default,
        with = "arc_u16_le_base64",
        skip_serializing_if = "arc_u16_is_empty"
    )]
    pub rgb_scene16f: Arc<[u16]>,
    #[serde(
        default,
        with = "arc_u8_base64",
        skip_serializing_if = "arc_u8_is_empty"
    )]
    pub alpha: Arc<[u8]>,
    #[serde(skip)]
    pub(crate) sidecar_cache: Arc<OnceLock<RemovePatchSidecarCache>>,
}

fn arc_u16_is_empty(values: &Arc<[u16]>) -> bool {
    values.is_empty()
}

fn arc_u8_is_empty(values: &Arc<[u8]>) -> bool {
    values.is_empty()
}

impl PartialEq for RemovePatch {
    fn eq(&self, other: &Self) -> bool {
        self.bounds == other.bounds
            && self.rgb_scene16f == other.rgb_scene16f
            && self.alpha == other.alpha
    }
}

impl RemovePatch {
    pub fn new_scene(
        bounds: NativeRect,
        rgb_scene16f: Vec<u16>,
        alpha: Vec<u8>,
    ) -> Result<Self, &'static str> {
        let pixels = (bounds.width as usize)
            .checked_mul(bounds.height as usize)
            .ok_or("remove patch pixel count overflows")?;
        if pixels == 0 {
            return Err("remove patch is empty");
        }
        if rgb_scene16f.len() != pixels.saturating_mul(3) {
            return Err("remove scene RGB length does not match bounds");
        }
        if alpha.len() != pixels {
            return Err("remove patch alpha length does not match bounds");
        }
        Ok(Self {
            bounds,
            rgb_scene16f: Arc::from(rgb_scene16f),
            alpha: Arc::from(alpha),
            sidecar_cache: Arc::default(),
        })
    }

    pub fn has_scene_pixels(&self) -> bool {
        !self.rgb_scene16f.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RemoveStroke {
    pub brush: RemoveBrushStroke,
    #[serde(default)]
    pub patches: Vec<RemovePatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retouch: Option<RetouchStroke>,
    #[serde(default = "default_remove_stroke_opacity")]
    pub opacity: f32,
}

impl Default for RemoveStroke {
    fn default() -> Self {
        Self {
            brush: RemoveBrushStroke::default(),
            patches: Vec::new(),
            retouch: None,
            opacity: 1.0,
        }
    }
}

const fn default_remove_stroke_opacity() -> f32 {
    1.0
}

impl RemoveStroke {
    pub fn composite_opacity(&self) -> f32 {
        self.opacity.clamp(0.0, 1.0)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RemoveEditState {
    #[serde(default)]
    pub strokes: Vec<RemoveStroke>,
}

impl RemoveEditState {
    pub fn is_empty(&self) -> bool {
        self.strokes.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoveMask {
    pub bounds: NativeRect,
    pub pixels: Vec<u8>,
}

impl RemoveMask {
    pub fn is_empty(&self) -> bool {
        self.pixels.iter().all(|value| *value == 0)
    }

    /// Number of masked pixels inside `region`.
    fn count_in(&self, region: NativeRect) -> usize {
        let Some(overlap) = self.bounds.intersect(region) else {
            return 0;
        };
        let width = self.bounds.width as usize;
        (overlap.y..overlap.bottom())
            .map(|y| {
                let row = (y - self.bounds.y) as usize * width;
                let start = row + (overlap.x - self.bounds.x) as usize;
                self.pixels[start..start + overlap.width as usize]
                    .iter()
                    .filter(|value| **value != 0)
                    .count()
            })
            .sum()
    }

    fn masked_fraction(&self, region: NativeRect) -> f32 {
        let area = region.width as usize * region.height as usize;
        self.count_in(region) as f32 / area.max(1) as f32
    }

    /// Masked pixels inside `region`, with bounds shrunk to fit them.
    fn restricted_to(&self, region: NativeRect) -> Option<Self> {
        let overlap = self.bounds.intersect(region)?;
        let mut pixels = vec![0u8; overlap.width as usize * overlap.height as usize];
        for y in overlap.y..overlap.bottom() {
            for x in overlap.x..overlap.right() {
                if self.contains_global(x, y) {
                    pixels[(y - overlap.y) as usize * overlap.width as usize
                        + (x - overlap.x) as usize] = 255;
                }
            }
        }
        Self {
            bounds: overlap,
            pixels,
        }
        .shrunk()
    }

    /// Clears every pixel that is set in `other`.
    pub fn subtract(&mut self, other: &Self) {
        let Some(overlap) = self.bounds.intersect(other.bounds) else {
            return;
        };
        for y in overlap.y..overlap.bottom() {
            for x in overlap.x..overlap.right() {
                if other.contains_global(x, y) {
                    let index = (y - self.bounds.y) as usize * self.bounds.width as usize
                        + (x - self.bounds.x) as usize;
                    self.pixels[index] = 0;
                }
            }
        }
    }

    /// Separate spots of the mask (8-connected), each with tight bounds.
    fn connected_components(&self) -> Vec<Self> {
        let width = self.bounds.width as usize;
        let height = self.bounds.height as usize;
        // 0 = unvisited; otherwise the 1-based component number.
        let mut labels = vec![0u32; self.pixels.len()];
        let mut component_bounds: Vec<[usize; 4]> = Vec::new();
        let mut stack = Vec::new();
        for start in 0..self.pixels.len() {
            if labels[start] != 0 || self.pixels[start] == 0 {
                continue;
            }
            let label = component_bounds.len() as u32 + 1;
            let mut bounds = [usize::MAX, usize::MAX, 0, 0];
            labels[start] = label;
            stack.push(start);
            while let Some(index) = stack.pop() {
                let (x, y) = (index % width, index / width);
                bounds = [
                    bounds[0].min(x),
                    bounds[1].min(y),
                    bounds[2].max(x + 1),
                    bounds[3].max(y + 1),
                ];
                for neighbor_y in y.saturating_sub(1)..(y + 2).min(height) {
                    for neighbor_x in x.saturating_sub(1)..(x + 2).min(width) {
                        let neighbor = neighbor_y * width + neighbor_x;
                        if labels[neighbor] == 0 && self.pixels[neighbor] != 0 {
                            labels[neighbor] = label;
                            stack.push(neighbor);
                        }
                    }
                }
            }
            component_bounds.push(bounds);
        }
        component_bounds
            .into_iter()
            .enumerate()
            .map(|(index, [left, top, right, bottom])| {
                let label = index as u32 + 1;
                let mut pixels = Vec::with_capacity((right - left) * (bottom - top));
                for y in top..bottom {
                    pixels.extend(
                        labels[y * width + left..y * width + right]
                            .iter()
                            .map(|value| if *value == label { 255 } else { 0 }),
                    );
                }
                Self {
                    bounds: NativeRect {
                        x: self.bounds.x + left as u32,
                        y: self.bounds.y + top as u32,
                        width: (right - left) as u32,
                        height: (bottom - top) as u32,
                    },
                    pixels,
                }
            })
            .collect()
    }

    /// Largest distance in native pixels from a masked pixel to the nearest
    /// unmasked one: half the stroke's thickness at its widest.
    fn max_half_width(&self) -> f32 {
        const ORTHOGONAL: u32 = 3;
        const DIAGONAL: u32 = 4;
        // Pad by one pixel so the bounds' border counts as unmasked.
        let width = self.bounds.width as usize + 2;
        let height = self.bounds.height as usize + 2;
        let mut distance = vec![0u32; width * height];
        for y in 0..self.bounds.height as usize {
            for x in 0..self.bounds.width as usize {
                if self.pixels[y * self.bounds.width as usize + x] != 0 {
                    distance[(y + 1) * width + x + 1] = u32::MAX / 2;
                }
            }
        }
        let neighbors = [
            (-1isize, 0isize, ORTHOGONAL),
            (0, -1, ORTHOGONAL),
            (-1, -1, DIAGONAL),
            (1, -1, DIAGONAL),
        ];
        for y in 1..height - 1 {
            for x in 1..width - 1 {
                let index = y * width + x;
                for (dx, dy, step) in neighbors {
                    let neighbor = (index as isize + dy * width as isize + dx) as usize;
                    distance[index] = distance[index].min(distance[neighbor] + step);
                }
            }
        }
        let mut maximum = 0;
        for y in (1..height - 1).rev() {
            for x in (1..width - 1).rev() {
                let index = y * width + x;
                for (dx, dy, step) in neighbors {
                    let neighbor = (index as isize - dy * width as isize - dx) as usize;
                    distance[index] = distance[index].min(distance[neighbor] + step);
                }
                maximum = maximum.max(distance[index]);
            }
        }
        maximum as f32 / ORTHOGONAL as f32
    }

    /// The mask split into square tiles of `edge`, dropping empty ones.
    fn tiles(&self, edge: u32) -> Vec<Self> {
        let edge = edge.max(1);
        let mut tiles = Vec::new();
        let mut y = self.bounds.y;
        while y < self.bounds.bottom() {
            let mut x = self.bounds.x;
            while x < self.bounds.right() {
                let tile = NativeRect {
                    x,
                    y,
                    width: edge.min(self.bounds.right() - x),
                    height: edge.min(self.bounds.bottom() - y),
                };
                tiles.extend(self.restricted_to(tile));
                x += edge;
            }
            y += edge;
        }
        tiles
    }

    /// The same mask with bounds shrunk to its set pixels; `None` when empty.
    fn shrunk(self) -> Option<Self> {
        let width = self.bounds.width as usize;
        let (mut left, mut top, mut right, mut bottom) = (usize::MAX, usize::MAX, 0, 0);
        for (index, value) in self.pixels.iter().enumerate() {
            if *value != 0 {
                let (x, y) = (index % width, index / width);
                left = left.min(x);
                top = top.min(y);
                right = right.max(x + 1);
                bottom = bottom.max(y + 1);
            }
        }
        if right <= left || bottom <= top {
            return None;
        }
        let bounds = NativeRect {
            x: self.bounds.x + left as u32,
            y: self.bounds.y + top as u32,
            width: (right - left) as u32,
            height: (bottom - top) as u32,
        };
        let mut pixels = Vec::with_capacity((right - left) * (bottom - top));
        for y in top..bottom {
            pixels.extend_from_slice(&self.pixels[y * width + left..y * width + right]);
        }
        Some(Self { bounds, pixels })
    }

    pub fn contains_global(&self, x: u32, y: u32) -> bool {
        if x < self.bounds.x
            || y < self.bounds.y
            || x >= self.bounds.right()
            || y >= self.bounds.bottom()
        {
            return false;
        }
        let local_x = (x - self.bounds.x) as usize;
        let local_y = (y - self.bounds.y) as usize;
        self.pixels[local_y * self.bounds.width as usize + local_x] != 0
    }
}

mod arc_u8_base64 {
    use super::*;
    use base64::Engine as _;

    pub(super) fn serialize<S>(bytes: &Arc<[u8]>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes.as_ref()))
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Arc<[u8]>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map(Arc::from)
            .map_err(serde::de::Error::custom)
    }
}

mod arc_u16_le_base64 {
    use super::*;
    use base64::Engine as _;

    pub(super) fn serialize<S>(values: &Arc<[u16]>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut bytes = Vec::with_capacity(values.len().saturating_mul(2));
        for value in values.iter().copied() {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Arc<[u16]>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(serde::de::Error::custom)?;
        if bytes.len() % 2 != 0 {
            return Err(serde::de::Error::custom("RGB16 patch byte length is odd"));
        }
        let values = bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        Ok(Arc::from(values))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_brush_raster_is_hard_binary_before_dilation() {
        let brush = RemoveBrushStroke {
            points: vec![RemoveBrushPoint {
                x: 32.0,
                y: 24.0,
                radius: 7.5,
            }],
            dilation_radius: 0,
        };
        let mask = rasterize_remove_brush(64, 48, &brush).unwrap();
        assert!(mask.pixels.iter().all(|value| *value == 0 || *value == 255));
        assert!(mask.pixels.contains(&255));
        assert!(mask.pixels.contains(&0));
    }

    fn disc_mask(width: u32, height: u32, discs: &[(f32, f32, f32)]) -> RemoveMask {
        let brush = RemoveBrushStroke {
            points: discs
                .iter()
                .map(|&(x, y, radius)| RemoveBrushPoint { x, y, radius })
                .collect(),
            dilation_radius: 0,
        };
        rasterize_remove_brush(width, height, &brush).unwrap()
    }

    /// Every stroke pixel is filled by exactly one pass, inside its crop.
    fn assert_passes_partition(mask: &RemoveMask, passes: &[RemovePass]) {
        let mut remaining = mask.clone();
        for pass in passes {
            assert_eq!(
                pass.crop.intersect(pass.target.bounds),
                Some(pass.target.bounds)
            );
            let before = remaining.count_in(mask.bounds);
            remaining.subtract(&pass.target);
            assert_eq!(
                before - remaining.count_in(mask.bounds),
                pass.target.count_in(pass.target.bounds),
                "a pass refills pixels an earlier pass filled"
            );
        }
        assert!(remaining.is_empty(), "some stroke pixels are never filled");
    }

    #[test]
    fn disjoint_rectangles_do_not_intersect() {
        let left = NativeRect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let right = NativeRect {
            x: 20,
            y: 0,
            width: 10,
            height: 10,
        };
        assert_eq!(left.intersect(right), None);
        assert_eq!(right.intersect(left), None);
    }

    #[test]
    fn small_spot_is_inferred_at_native_resolution() {
        let mask = disc_mask(7000, 6000, &[(3500.0, 3000.0, 60.0)]);
        let passes = plan_remove_passes(7000, 6000, &mask);
        assert_eq!(passes.len(), 1);
        assert_eq!(passes[0].crop.width, BIG_LAMA_INPUT_EDGE);
        assert_eq!(passes[0].crop.height, BIG_LAMA_INPUT_EDGE);
        assert_passes_partition(&mask, &passes);
    }

    #[test]
    fn separate_spots_get_their_own_native_passes() {
        let mask = disc_mask(7000, 6000, &[(800.0, 900.0, 40.0), (5200.0, 4100.0, 50.0)]);
        let passes = plan_remove_passes(7000, 6000, &mask);
        assert_eq!(passes.len(), 2);
        assert!(passes
            .iter()
            .all(|pass| pass.crop.width == BIG_LAMA_INPUT_EDGE));
        assert_passes_partition(&mask, &passes);
    }

    #[test]
    fn thin_long_stroke_is_tiled_at_native_resolution() {
        let wire = (0..60)
            .map(|step| (500.0 + step as f32 * 40.0, 2000.0 + step as f32 * 10.0, 6.0))
            .collect::<Vec<_>>();
        let mask = disc_mask(7000, 6000, &wire);
        let passes = plan_remove_passes(7000, 6000, &mask);
        assert!(passes.len() > 1);
        for pass in &passes {
            assert_eq!(pass.crop.width, BIG_LAMA_INPUT_EDGE);
            assert!(mask.masked_fraction(pass.crop) <= MAX_MASKED_FRACTION);
        }
        assert_passes_partition(&mask, &passes);
    }

    #[test]
    fn tall_object_is_filled_whole_instead_of_tiled() {
        let mask = RemoveMask {
            bounds: NativeRect {
                x: 600,
                y: 2000,
                width: 220,
                height: 600,
            },
            pixels: vec![255; 220 * 600],
        };
        assert!(mask.max_half_width() > MAX_TILED_HALF_WIDTH);
        let passes = plan_remove_passes(7000, 6000, &mask);
        assert_eq!(passes.len(), 1);
        assert!(passes[0].crop.width > BIG_LAMA_INPUT_EDGE);
        assert_passes_partition(&mask, &passes);
    }

    #[test]
    fn half_width_measures_stroke_thickness() {
        let wire = disc_mask(2000, 1000, &[(100.0, 500.0, 6.0), (1900.0, 500.0, 6.0)]);
        assert!(wire.max_half_width() < 8.0);
        let blob = disc_mask(2000, 1000, &[(1000.0, 500.0, 100.0)]);
        assert!((blob.max_half_width() - 100.0).abs() < 10.0);
    }

    #[test]
    fn large_object_keeps_context_in_one_downscaled_pass() {
        let mask = RemoveMask {
            bounds: NativeRect {
                x: 1000,
                y: 1000,
                width: 1200,
                height: 900,
            },
            pixels: vec![255; 1200 * 900],
        };
        let passes = plan_remove_passes(7000, 6000, &mask);
        assert_eq!(passes.len(), 1);
        let crop = passes[0].crop;
        assert_eq!(crop.width, crop.height);
        assert!(crop.width > BIG_LAMA_INPUT_EDGE);
        assert!(mask.masked_fraction(crop) <= MAX_MASKED_FRACTION);
        assert_passes_partition(&mask, &passes);
    }

    #[test]
    fn mask_wider_than_the_image_uses_the_full_image_once() {
        let mask = RemoveMask {
            bounds: NativeRect {
                x: 0,
                y: 100,
                width: 1200,
                height: 300,
            },
            pixels: vec![255; 1200 * 300],
        };
        let passes = plan_remove_passes(1200, 1000, &mask);
        assert_eq!(passes.len(), 1);
        assert_eq!(
            passes[0].crop,
            NativeRect {
                x: 0,
                y: 0,
                width: 1200,
                height: 1000,
            }
        );
    }

    #[test]
    fn components_and_restriction_keep_global_coordinates() {
        let mask = disc_mask(400, 300, &[(50.0, 50.0, 10.0), (300.0, 200.0, 12.0)]);
        let components = mask.connected_components();
        assert_eq!(components.len(), 2);
        assert_eq!(
            components
                .iter()
                .map(|c| c.count_in(c.bounds))
                .sum::<usize>(),
            mask.count_in(mask.bounds)
        );
        let left = mask
            .restricted_to(NativeRect {
                x: 0,
                y: 0,
                width: 200,
                height: 300,
            })
            .unwrap();
        assert_eq!(left, components[0]);
    }

    #[test]
    fn remove_model_view_round_trips_scene_linear_raster() {
        let raw = LoadedRaw::from_scene_linear_rec2020(1, 1, vec![0.12, 0.18, 0.09]).unwrap();
        let exposure = ExposureParams::default();
        let scene = [0.12, 0.18, 0.09];
        let view_gain = 1.75;
        let model = remove_scene_to_model_srgb(&raw, scene, view_gain);
        let restored = remove_model_srgb_to_canonical_scene(&raw, &exposure, model, view_gain);
        for channel in 0..3 {
            assert!(
                (restored[channel] - scene[channel]).abs() < 2e-4,
                "channel {channel}: scene={} restored={}",
                scene[channel],
                restored[channel]
            );
        }
    }

    #[test]
    fn compositing_does_not_touch_outside_patch() {
        let patch = RemovePatch::new_scene(
            NativeRect {
                x: 1,
                y: 1,
                width: 1,
                height: 1,
            },
            vec![
                half::f16::from_f32(1.0).to_bits(),
                half::f16::from_f32(0.0).to_bits(),
                half::f16::from_f32(0.0).to_bits(),
            ],
            vec![255],
        )
        .unwrap();
        let mut rgb = vec![0.25f32; 3 * 3 * 3];
        let before = rgb.clone();
        composite_patch_into_linear_region(
            &patch,
            NativeRect {
                x: 0,
                y: 0,
                width: 3,
                height: 3,
            },
            &mut rgb,
        );
        for pixel in 0..9 {
            if pixel != 4 {
                assert_eq!(
                    &rgb[pixel * 3..pixel * 3 + 3],
                    &before[pixel * 3..pixel * 3 + 3]
                );
            }
        }
        assert_ne!(&rgb[12..15], &before[12..15]);
    }

    #[test]
    fn stroke_opacity_is_live_for_remove_and_current_retouch_patches() {
        let remove = RemoveStroke {
            opacity: 0.35,
            ..RemoveStroke::default()
        };
        assert_eq!(remove.composite_opacity(), 0.35);

        let retouch = RemoveStroke {
            opacity: 0.4,
            retouch: Some(RetouchStroke {
                tool: RetouchTool::Clone,
                alignment: RetouchAlignment::Aligned,
                source: [0.0; 2],
                destination: [0.0; 2],
                hardness: 0.5,
                opacity: 0.8,
            }),
            ..RemoveStroke::default()
        };
        assert_eq!(retouch.composite_opacity(), 0.4);
    }
}
