use super::{color_profile::display_linear_rec2020_to_srgb, ExposureParams, LoadedRaw};
use crate::color_math::{linear_srgb_to_rec2020, srgb_decode};
use crate::matrix;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::sync::{Arc, OnceLock};

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

pub fn adaptive_remove_dilation(stroke: &[RemoveBrushPoint]) -> u32 {
    if stroke.is_empty() {
        return 0;
    }
    let average_radius = stroke
        .iter()
        .map(|point| point.radius.max(0.0))
        .sum::<f32>()
        / stroke.len() as f32;
    (average_radius * 0.06).round().clamp(1.0, 12.0) as u32
}

pub fn rasterize_remove_brush(
    image_width: u32,
    image_height: u32,
    brush: &RemoveBrushStroke,
) -> Option<RemoveMask> {
    if image_width == 0 || image_height == 0 || brush.points.is_empty() {
        return None;
    }
    let dilation = brush.dilation_radius as f32;
    let mut x0 = image_width as f32;
    let mut y0 = image_height as f32;
    let mut x1 = 0.0f32;
    let mut y1 = 0.0f32;
    for point in &brush.points {
        if !point.x.is_finite() || !point.y.is_finite() || !point.radius.is_finite() {
            continue;
        }
        let radius = point.radius.max(0.5) + dilation;
        x0 = x0.min(point.x - radius - 1.0);
        y0 = y0.min(point.y - radius - 1.0);
        x1 = x1.max(point.x + radius + 1.0);
        y1 = y1.max(point.y + radius + 1.0);
    }
    let left = x0.floor().max(0.0).min(image_width as f32) as u32;
    let top = y0.floor().max(0.0).min(image_height as f32) as u32;
    let right = x1.ceil().max(0.0).min(image_width as f32) as u32;
    let bottom = y1.ceil().max(0.0).min(image_height as f32) as u32;
    if right <= left || bottom <= top {
        return None;
    }
    let bounds = NativeRect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    };
    let mut pixels = vec![0u8; bounds.width as usize * bounds.height as usize];

    for point in &brush.points {
        let radius = point.radius.max(0.5) + dilation;
        paint_disc(&mut pixels, bounds, point.x, point.y, radius);
    }

    Some(RemoveMask { bounds, pixels })
}

fn paint_disc(pixels: &mut [u8], bounds: NativeRect, center_x: f32, center_y: f32, radius: f32) {
    let y_start = (center_y - radius)
        .floor()
        .max(bounds.y as f32)
        .min(bounds.bottom() as f32) as u32;
    let y_end = (center_y + radius)
        .ceil()
        .max(bounds.y as f32)
        .min(bounds.bottom() as f32) as u32;
    let radius_sq = radius * radius;
    for y in y_start..y_end {
        let dy = y as f32 + 0.5 - center_y;
        let remaining = (radius_sq - dy * dy).max(0.0).sqrt();
        let x_start = (center_x - remaining)
            .floor()
            .max(bounds.x as f32)
            .min(bounds.right() as f32) as u32;
        let x_end = (center_x + remaining)
            .ceil()
            .max(bounds.x as f32)
            .min(bounds.right() as f32) as u32;
        let row = (y - bounds.y) as usize * bounds.width as usize;
        for x in x_start..x_end {
            pixels[row + (x - bounds.x) as usize] = 255;
        }
    }
}

/// Native pixels a Big-LaMa pass sends to the model: exactly its fixed input,
/// so a pass of this size is inferred without any resampling.
const NATIVE_PASS_EDGE: u32 = BIG_LAMA_INPUT_EDGE;
/// Context kept around a target before it no longer fits a native pass.
const MIN_CONTEXT_MARGIN: u32 = 64;
/// Thin strokes too long for one native pass are filled in tiles of this edge.
const REMOVE_TILE_EDGE: u32 = 320;
/// Only strokes at most this many native pixels from edge to centre are thin
/// enough to tile: every tile then still sees background right beside the
/// stroke. Thicker objects are filled whole so no tile cuts through them.
const MAX_TILED_HALF_WIDTH: f32 = 40.0;
/// Beyond this many tiles a stroke is filled in fewer, downscaled passes;
/// each pass costs about a second and a half of CPU inference.
const MAX_TILED_PASSES: usize = 32;
/// Largest share of a crop that may be masked; the rest is the context the
/// model fills from.
const MAX_MASKED_FRACTION: f32 = 0.4;
/// Context edge of a downscaled pass relative to its target's edge.
const DOWNSCALED_CONTEXT_FACTOR: f32 = 1.75;

/// One Big-LaMa inference within a Remove stroke.
///
/// `target` holds the mask pixels this pass fills. Stroke pixels inside `crop`
/// that later passes fill stay masked, so the model never copies the object
/// being removed, while pixels filled by earlier passes are real context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemovePass {
    pub crop: NativeRect,
    pub target: RemoveMask,
}

/// Splits a stroke into the passes that fill it at the best resolution the
/// fixed 512px model allows: each separate spot on its own, thin strokes in
/// native-resolution tiles, and larger objects in one downscaled pass with
/// enough surrounding context.
pub fn plan_remove_passes(
    image_width: u32,
    image_height: u32,
    mask: &RemoveMask,
) -> Vec<RemovePass> {
    if image_width == 0 || image_height == 0 || mask.is_empty() {
        return Vec::new();
    }
    let components = mask.connected_components();
    let tiled = components
        .iter()
        .flat_map(|component| {
            native_passes(image_width, image_height, mask, component).unwrap_or_else(|| {
                vec![downscaled_pass(image_width, image_height, mask, component)]
            })
        })
        .collect::<Vec<_>>();
    let native_count = tiled
        .iter()
        .filter(|pass| pass.crop.width.max(pass.crop.height) <= NATIVE_PASS_EDGE)
        .count();
    if native_count <= MAX_TILED_PASSES && tiled.len() <= REMOVE_MAX_PATCHES_PER_STROKE {
        return tiled;
    }
    let per_component = components
        .iter()
        .map(|component| downscaled_pass(image_width, image_height, mask, component))
        .collect::<Vec<_>>();
    if per_component.len() <= REMOVE_MAX_PATCHES_PER_STROKE {
        per_component
    } else {
        vec![downscaled_pass(image_width, image_height, mask, mask)]
    }
}

/// Native-resolution passes for a target, or `None` when one of them would
/// not keep enough unmasked context.
fn native_passes(
    image_width: u32,
    image_height: u32,
    stroke: &RemoveMask,
    target: &RemoveMask,
) -> Option<Vec<RemovePass>> {
    let target_edge = target.bounds.width.max(target.bounds.height);
    let tiles = if target_edge + 2 * MIN_CONTEXT_MARGIN <= NATIVE_PASS_EDGE {
        vec![target.clone()]
    } else if target.max_half_width() <= MAX_TILED_HALF_WIDTH {
        target.tiles(REMOVE_TILE_EDGE)
    } else {
        return None;
    };
    if tiles.len() > MAX_TILED_PASSES {
        return None;
    }
    tiles
        .into_iter()
        .map(|tile| {
            let crop = square_around(image_width, image_height, tile.bounds, NATIVE_PASS_EDGE);
            (stroke.masked_fraction(crop) <= MAX_MASKED_FRACTION)
                .then_some(RemovePass { crop, target: tile })
        })
        .collect()
}

fn downscaled_pass(
    image_width: u32,
    image_height: u32,
    stroke: &RemoveMask,
    target: &RemoveMask,
) -> RemovePass {
    let shortest = image_width.min(image_height).max(1);
    let target_edge = target.bounds.width.max(target.bounds.height).max(1);
    if target_edge > shortest {
        // A square crop cannot contain the target; use the complete image.
        return RemovePass {
            crop: NativeRect {
                x: 0,
                y: 0,
                width: image_width,
                height: image_height,
            },
            target: target.clone(),
        };
    }
    let mut edge = ((target_edge as f32 * DOWNSCALED_CONTEXT_FACTOR).ceil() as u32)
        .max(target_edge + 2 * MIN_CONTEXT_MARGIN)
        .max(NATIVE_PASS_EDGE)
        .min(shortest);
    let mut crop = square_around(image_width, image_height, target.bounds, edge);
    while stroke.masked_fraction(crop) > MAX_MASKED_FRACTION && edge < shortest {
        edge = (edge + edge / 4).min(shortest);
        crop = square_around(image_width, image_height, target.bounds, edge);
    }
    RemovePass {
        crop,
        target: target.clone(),
    }
}

fn square_around(image_width: u32, image_height: u32, bounds: NativeRect, edge: u32) -> NativeRect {
    square_inside_image(
        image_width,
        image_height,
        bounds.x + bounds.width / 2,
        bounds.y + bounds.height / 2,
        edge,
    )
}

fn square_inside_image(
    image_width: u32,
    image_height: u32,
    center_x: u32,
    center_y: u32,
    requested_edge: u32,
) -> NativeRect {
    let edge = requested_edge
        .max(1)
        .min(image_width.max(1))
        .min(image_height.max(1));
    let half = edge / 2;
    let mut x = center_x.saturating_sub(half);
    let mut y = center_y.saturating_sub(half);
    if x.saturating_add(edge) > image_width {
        x = image_width.saturating_sub(edge);
    }
    if y.saturating_add(edge) > image_height {
        y = image_height.saturating_sub(edge);
    }
    NativeRect {
        x,
        y,
        width: edge,
        height: edge,
    }
}

pub fn remove_scene_white_balance(raw: &LoadedRaw, exposure: &ExposureParams) -> [f32; 3] {
    if raw.is_pre_demosaiced_raster() {
        return [1.0; 3];
    }
    let (wb, _, _) =
        raw.adjusted_white_balance_and_camera_transform(exposure.temperature, exposure.tint);
    let green = 0.5 * (wb[1] + wb[3]);
    [wb[0].max(1e-8), green.max(1e-8), wb[2].max(1e-8)]
}

pub fn pipeline_scene_to_canonical_remove_scene(
    raw: &LoadedRaw,
    exposure: &ExposureParams,
    rgb: [f32; 3],
) -> [f32; 3] {
    if raw.is_pre_demosaiced_raster() {
        return rgb;
    }
    let wb = remove_scene_white_balance(raw, exposure);
    [rgb[0] / wb[0], rgb[1] / wb[1], rgb[2] / wb[2]]
}

pub fn canonical_remove_scene_to_pipeline_scene(
    raw: &LoadedRaw,
    exposure: &ExposureParams,
    rgb: [f32; 3],
) -> [f32; 3] {
    if raw.is_pre_demosaiced_raster() {
        return rgb;
    }
    let wb = remove_scene_white_balance(raw, exposure);
    [rgb[0] * wb[0], rgb[1] * wb[1], rgb[2] * wb[2]]
}

pub fn pipeline_scene_to_working_rec2020(raw: &LoadedRaw, rgb: [f32; 3]) -> [f32; 3] {
    if raw.is_pre_demosaiced_raster() && !raw.is_camera_linear_raster() {
        return rgb;
    }
    let m = remove_camera_transform(raw);
    [
        m[0][0] * rgb[0] + m[0][1] * rgb[1] + m[0][2] * rgb[2],
        m[1][0] * rgb[0] + m[1][1] * rgb[1] + m[1][2] * rgb[2],
        m[2][0] * rgb[0] + m[2][1] * rgb[1] + m[2][2] * rgb[2],
    ]
}

// Sensor pipeline scenes have WB applied already; camera rasters do not.
fn remove_camera_transform(raw: &LoadedRaw) -> [[f32; 4]; 3] {
    let mut transform = raw.cam_to_srgb;
    if raw.is_camera_linear_raster() {
        for row in &mut transform {
            for (value, gain) in row.iter_mut().zip(raw.wb_coeffs) {
                *value *= gain;
            }
        }
    }
    transform
}

fn invert_remove_camera_matrix(raw: &LoadedRaw) -> Option<[[f32; 3]; 3]> {
    if raw.is_pre_demosaiced_raster() && !raw.is_camera_linear_raster() {
        return Some(matrix::IDENTITY3);
    }
    let rgb_columns = remove_camera_transform(raw).map(|row| [row[0], row[1], row[2]]);
    matrix::invert(rgb_columns)
}

pub fn working_rec2020_to_canonical_remove_scene(
    raw: &LoadedRaw,
    exposure: &ExposureParams,
    rgb: [f32; 3],
) -> [f32; 3] {
    if raw.is_pre_demosaiced_raster() && !raw.is_camera_linear_raster() {
        return rgb;
    }
    let Some(m) = invert_remove_camera_matrix(raw) else {
        return rgb;
    };
    let camera_wb = [
        m[0][0] * rgb[0] + m[0][1] * rgb[1] + m[0][2] * rgb[2],
        m[1][0] * rgb[0] + m[1][1] * rgb[1] + m[1][2] * rgb[2],
        m[2][0] * rgb[0] + m[2][1] * rgb[1] + m[2][2] * rgb[2],
    ];
    pipeline_scene_to_canonical_remove_scene(raw, exposure, camera_wb)
}

pub fn remove_scene_to_model_srgb(
    raw: &LoadedRaw,
    scene_rgb: [f32; 3],
    view_gain: f32,
) -> [f32; 3] {
    let working = pipeline_scene_to_working_rec2020(raw, scene_rgb);
    let scaled = working.map(|value| value.max(0.0) * view_gain.max(1e-6));
    let luma = (scaled[0] * 0.2627 + scaled[1] * 0.6780 + scaled[2] * 0.0593).max(0.0);
    let shoulder = 1.0 / (1.0 + luma);
    display_linear_rec2020_to_model_srgb(scaled.map(|value| value * shoulder))
}

pub fn remove_model_srgb_to_canonical_scene(
    raw: &LoadedRaw,
    exposure: &ExposureParams,
    srgb: [f32; 3],
    view_gain: f32,
) -> [f32; 3] {
    let mapped = model_srgb_to_display_linear_rec2020(srgb);
    let mapped_luma =
        (mapped[0] * 0.2627 + mapped[1] * 0.6780 + mapped[2] * 0.0593).clamp(0.0, 0.985);
    let undo_shoulder = 1.0 / (1.0 - mapped_luma).max(0.015);
    let gain = view_gain.max(1e-6);
    let working = mapped.map(|value| value * undo_shoulder / gain);
    working_rec2020_to_canonical_remove_scene(raw, exposure, working)
}

pub fn remove_model_view_gain(raw: &LoadedRaw, scene_rgb: &[f32]) -> f32 {
    let mut luminance = Vec::new();
    for pixel in scene_rgb.chunks_exact(3).step_by(4) {
        let working = pipeline_scene_to_working_rec2020(raw, [pixel[0], pixel[1], pixel[2]]);
        let value = working[0] * 0.2627 + working[1] * 0.6780 + working[2] * 0.0593;
        if value.is_finite() && value > 1e-6 {
            luminance.push(value.min(64.0));
        }
    }
    if luminance.is_empty() {
        return 1.0;
    }
    luminance.sort_by(f32::total_cmp);
    let index = ((luminance.len() - 1) as f32 * 0.75).round() as usize;
    let p75 = luminance[index].max(1e-5);
    // Place the upper quartile like a normally exposed photo (about 0.63 in
    // sRGB), the kind of image Big-LaMa was trained on.
    let target_display = 0.35;
    let target_linear = target_display / (1.0 - target_display);
    (target_linear / p75).clamp(0.25, 64.0)
}

pub fn composite_remove_edits_into_linear_region(
    edits: &RemoveEditState,
    region: NativeRect,
    rgb: &mut [f32],
) {
    if rgb.len() != region.width as usize * region.height as usize * 3 {
        return;
    }
    for stroke in &edits.strokes {
        for patch in &stroke.patches {
            composite_patch_into_linear_region_with_opacity(
                patch,
                region,
                rgb,
                stroke.composite_opacity(),
                stroke.retouch.is_some(),
            );
        }
    }
}

pub fn composite_patch_into_linear_region(
    patch: &RemovePatch,
    region: NativeRect,
    rgb: &mut [f32],
) {
    composite_patch_into_linear_region_with_opacity(patch, region, rgb, 1.0, false);
}

fn composite_patch_into_linear_region_with_opacity(
    patch: &RemovePatch,
    region: NativeRect,
    rgb: &mut [f32],
    opacity: f32,
    retouch_coverage: bool,
) {
    if !patch.has_scene_pixels() {
        return;
    }
    let Some(intersection) = patch.bounds.intersect(region) else {
        return;
    };
    for y in intersection.y..intersection.bottom() {
        let patch_y = (y - patch.bounds.y) as usize;
        let region_y = (y - region.y) as usize;
        for x in intersection.x..intersection.right() {
            let patch_x = (x - patch.bounds.x) as usize;
            let region_x = (x - region.x) as usize;
            let patch_index = patch_y * patch.bounds.width as usize + patch_x;
            let coverage = patch.alpha[patch_index] as f32 / 255.0;
            let alpha = if retouch_coverage {
                if coverage > 0.0 {
                    opacity
                } else {
                    0.0
                }
            } else {
                coverage * opacity
            };
            if alpha <= 0.0 {
                continue;
            }
            let rgb_index = patch_index * 3;
            let repaired = [
                half::f16::from_bits(patch.rgb_scene16f[rgb_index]).to_f32(),
                half::f16::from_bits(patch.rgb_scene16f[rgb_index + 1]).to_f32(),
                half::f16::from_bits(patch.rgb_scene16f[rgb_index + 2]).to_f32(),
            ];
            let out_index = (region_y * region.width as usize + region_x) * 3;
            for channel in 0..3 {
                rgb[out_index + channel] =
                    rgb[out_index + channel] * (1.0 - alpha) + repaired[channel] * alpha;
            }
        }
    }
}

pub fn display_linear_rec2020_to_model_srgb(rgb: [f32; 3]) -> [f32; 3] {
    display_linear_rec2020_to_srgb(rgb)
}

pub fn model_srgb_to_display_linear_rec2020(rgb: [f32; 3]) -> [f32; 3] {
    linear_srgb_to_rec2020(rgb.map(srgb_decode))
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
