//! Whether an export is marked as AI-edited.
//!
//! The EU AI Act (Article 50(2)) has providers of generative AI systems mark
//! content the system substantially altered, in a machine-readable way. It
//! exempts minor edits: the Commission's guidelines (C(2026) 5054, section
//! 4.3) list the removal of dust spots, obscuring backgrounds and blurring
//! faces as exempt, and the removal of objects or persons that changes what a
//! photo shows as marked. CalibRaw draws that line by how much of the exported
//! frame Big-LaMa filled: scattered small fills (dust, specks, a number plate)
//! stay unmarked, a large region or several medium ones mark the export.
//! Clone and Heal are not AI and never count.
//!
//! The thresholds are shares of the exported frame (after crop, rotation,
//! perspective and lens correction), so a tight crop around a fill counts it
//! at the size it is shown.

use super::{RemoveEditState, RemovePatch};
use crate::pipeline::geometry::GeometryInverseMap;
use crate::pipeline::{GeometryTransform, LoadedRaw};

/// Long edge of the grid the exported frame is sampled on, in cells.
const GRID_EDGE: u32 = 256;
/// A region below this share of the frame is a minor cleanup and ignored.
const MINOR_REGION_SHARE: f32 = 0.003;
/// One region of at least this share of the frame marks the export.
const LARGE_REGION_SHARE: f32 = 0.02;
/// Regions that are not minor mark the export once they cover this share
/// together.
const TOTAL_SHARE: f32 = 0.03;
/// A source pixel counts as filled where the fill's weight (patch alpha times
/// stroke opacity, 0–1) reaches this, so feathered edges and faint strokes
/// do not count.
const FILLED_WEIGHT: f32 = 0.5;

/// Whether an export of `raw` through `geometry` with `edits` is marked as
/// AI-edited: see the module documentation.
pub fn is_ai_edited(edits: &RemoveEditState, raw: &LoadedRaw, geometry: GeometryTransform) -> bool {
    let fills: Vec<(&RemovePatch, f32)> = edits
        .strokes
        .iter()
        .filter(|stroke| stroke.retouch.is_none() && stroke.composite_opacity() >= FILLED_WEIGHT)
        .flat_map(|stroke| {
            let opacity = stroke.composite_opacity();
            stroke
                .patches
                .iter()
                .filter(|patch| patch.has_scene_pixels())
                .map(move |patch| (patch, opacity))
        })
        .collect();
    if fills.is_empty() {
        return false;
    }
    let geometry = geometry.sanitized();
    let (output_width, output_height) = geometry.crop_pixel_dimensions(raw.width, raw.height);
    let map = GeometryInverseMap::new_with_lens(
        geometry,
        raw.lens_geometry.as_deref(),
        raw.width,
        raw.height,
        output_width,
        output_height,
    );
    let [output_width, output_height] = map.output_size();
    let scale = (GRID_EDGE as f32 / output_width.max(output_height) as f32).min(1.0);
    let columns = ((output_width as f32 * scale).round() as u32).max(1);
    let rows = ((output_height as f32 * scale).round() as u32).max(1);
    let mut filled = Vec::with_capacity(columns as usize * rows as usize);
    for row in 0..rows {
        for column in 0..columns {
            // Cell centres in output pixels (centres at half pixels), as
            // `GeometryInverseMap::source_position` takes them.
            let x = (column as f32 + 0.5) * output_width as f32 / columns as f32 - 0.5;
            let y = (row as f32 + 0.5) * output_height as f32 / rows as f32 - 0.5;
            let [source_x, source_y] = map.source_position(x, y);
            filled.push(is_filled(&fills, source_x, source_y));
        }
    }
    exceeds_thresholds(&filled, columns as usize, rows as usize)
}

/// Whether source pixel `(x, y)` (centres at whole pixels) is filled.
fn is_filled(fills: &[(&RemovePatch, f32)], x: f32, y: f32) -> bool {
    let (x, y) = (x.round(), y.round());
    if x < 0.0 || y < 0.0 {
        return false;
    }
    // In range after the check above; the casts saturate beyond u32.
    let (x, y) = (x as u32, y as u32);
    fills.iter().any(|(patch, opacity)| {
        let bounds = patch.bounds;
        if x < bounds.x || y < bounds.y || x >= bounds.right() || y >= bounds.bottom() {
            return false;
        }
        let index = (y - bounds.y) as usize * bounds.width as usize + (x - bounds.x) as usize;
        patch
            .alpha
            .get(index)
            .is_some_and(|alpha| f32::from(*alpha) / 255.0 * opacity >= FILLED_WEIGHT)
    })
}

/// Whether the filled cells of a `columns`×`rows` grid form a large region or
/// enough medium ones. Regions are 8-connected after a 3×3 dilation, so one
/// fill whose samples have small gaps stays one region; each region's area
/// counts only its filled cells.
fn exceeds_thresholds(filled: &[bool], columns: usize, rows: usize) -> bool {
    let total = (columns * rows) as f32;
    let neighbours = |index: usize| {
        let (column, row) = (index % columns, index / columns);
        let rows_range = row.saturating_sub(1)..=(row + 1).min(rows - 1);
        rows_range.flat_map(move |y| {
            (column.saturating_sub(1)..=(column + 1).min(columns - 1)).map(move |x| y * columns + x)
        })
    };
    let dilated: Vec<bool> = (0..filled.len())
        .map(|index| neighbours(index).any(|neighbour| filled[neighbour]))
        .collect();
    let mut visited = vec![false; filled.len()];
    let mut stack = Vec::new();
    let mut counted_share = 0.0;
    for start in 0..filled.len() {
        if !dilated[start] || visited[start] {
            continue;
        }
        visited[start] = true;
        stack.push(start);
        let mut cells = 0usize;
        while let Some(index) = stack.pop() {
            cells += usize::from(filled[index]);
            for neighbour in neighbours(index) {
                if dilated[neighbour] && !visited[neighbour] {
                    visited[neighbour] = true;
                    stack.push(neighbour);
                }
            }
        }
        let share = cells as f32 / total;
        if share < MINOR_REGION_SHARE {
            continue;
        }
        if share >= LARGE_REGION_SHARE {
            return true;
        }
        counted_share += share;
        if counted_share >= TOTAL_SHARE {
            return true;
        }
    }
    false
}
