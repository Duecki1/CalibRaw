//! Planning inpainting passes: native-resolution and downscaled crops.

use super::*;

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

/// Splits a stroke into texture synthesis passes: each separate spot with
/// the context around it, and thin strokes too long for one native context
/// in tiles with their own. Larger objects keep one pass, which is
/// synthesized at a reduced resolution when its context exceeds
/// [`FILL_MAX_NATIVE_CONTEXT_PIXELS`].
pub fn plan_fill_passes(image_width: u32, image_height: u32, mask: &RemoveMask) -> Vec<RemovePass> {
    if image_width == 0 || image_height == 0 || mask.is_empty() {
        return Vec::new();
    }
    let whole = |target: &RemoveMask| RemovePass {
        crop: fill_context(image_width, image_height, target.bounds),
        target: target.clone(),
    };
    let components = mask.connected_components();
    let passes = components
        .iter()
        .flat_map(|component| {
            let pass = whole(component);
            if pixel_count(pass.crop) <= FILL_MAX_NATIVE_CONTEXT_PIXELS
                || component.max_half_width() > MAX_TILED_HALF_WIDTH
            {
                vec![pass]
            } else {
                component.tiles(FILL_TILE_EDGE).iter().map(whole).collect()
            }
        })
        .collect::<Vec<_>>();
    if passes.len() <= REMOVE_MAX_PATCHES_PER_STROKE {
        passes
    } else if components.len() <= REMOVE_MAX_PATCHES_PER_STROKE {
        components.iter().map(whole).collect()
    } else {
        vec![whole(mask)]
    }
}

/// `bounds` grown by the fill's context margin on every side, within the image.
fn fill_context(image_width: u32, image_height: u32, bounds: NativeRect) -> NativeRect {
    let margin = bounds.width.max(bounds.height).max(FILL_MIN_CONTEXT_MARGIN);
    let x = bounds.x.saturating_sub(margin);
    let y = bounds.y.saturating_sub(margin);
    NativeRect {
        x,
        y,
        width: bounds.right().saturating_add(margin).min(image_width) - x,
        height: bounds.bottom().saturating_add(margin).min(image_height) - y,
    }
}

pub(super) fn pixel_count(rect: NativeRect) -> u64 {
    u64::from(rect.width) * u64::from(rect.height)
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
