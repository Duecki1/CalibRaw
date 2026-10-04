//! Point and box prompts sampled from user strokes, and the crop they define.

use super::*;

pub(super) fn strokes_extend(
    previous: &[crate::pipeline::ObjectStroke],
    current: &[crate::pipeline::ObjectStroke],
) -> bool {
    previous.len() <= current.len()
        && previous
            .iter()
            .zip(current.iter())
            .all(|(previous, current)| previous == current)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ObjectPromptKind {
    Foreground,
    Background,
    BoxTopLeft,
    BoxBottomRight,
}

impl ObjectPromptKind {
    pub(super) const fn sam_label(self) -> f32 {
        match self {
            Self::Foreground => 1.0,
            Self::Background => 0.0,
            Self::BoxTopLeft => 2.0,
            Self::BoxBottomRight => 3.0,
        }
    }

    pub(super) const fn is_foreground(self) -> bool {
        matches!(self, Self::Foreground)
    }

    pub(super) const fn is_background(self) -> bool {
        matches!(self, Self::Background)
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ObjectPrompt {
    pub(super) point: [f32; 2],
    pub(super) kind: ObjectPromptKind,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ObjectPromptFocus {
    pub(super) min: [f32; 2],
    pub(super) max: [f32; 2],
}

impl ObjectPromptFocus {
    fn contains(self, point: [f32; 2]) -> bool {
        point[0] >= self.min[0]
            && point[0] <= self.max[0]
            && point[1] >= self.min[1]
            && point[1] <= self.max[1]
    }
}

#[derive(Clone, Debug)]
pub(super) struct ObjectPromptSet {
    pub(super) prompts: Vec<ObjectPrompt>,
    pub(super) focus: ObjectPromptFocus,
}

pub(super) fn sampled_object_prompts(
    strokes: &[crate::pipeline::ObjectStroke],
    brush_size: f32,
    source_width: u32,
    source_height: u32,
    limit: usize,
) -> ObjectPromptSet {
    let mut foreground_points = Vec::new();
    let mut explicit_background_points = Vec::new();
    for stroke in strokes {
        let target = if stroke.positive {
            &mut foreground_points
        } else {
            &mut explicit_background_points
        };
        target.extend(
            stroke
                .points
                .iter()
                .map(|point| [point[0].clamp(0.0, 1.0), point[1].clamp(0.0, 1.0)]),
        );
    }

    let focus = object_prompt_focus(&foreground_points, brush_size, source_width, source_height);
    let foreground_budget = limit.saturating_sub(10).clamp(1, 16);
    let background_budget = limit.saturating_sub(foreground_budget + 2).min(6);
    let mut prompts = evenly_sample(&foreground_points, foreground_budget)
        .into_iter()
        .map(|point| ObjectPrompt {
            point,
            kind: ObjectPromptKind::Foreground,
        })
        .collect::<Vec<_>>();
    prompts.extend(
        evenly_sample(&explicit_background_points, background_budget)
            .into_iter()
            .map(|point| ObjectPrompt {
                point,
                kind: ObjectPromptKind::Background,
            }),
    );

    if prompts.len() + 2 <= limit {
        prompts.push(ObjectPrompt {
            point: focus.min,
            kind: ObjectPromptKind::BoxTopLeft,
        });
        prompts.push(ObjectPrompt {
            point: focus.max,
            kind: ObjectPromptKind::BoxBottomRight,
        });
    }

    let width = source_width.max(1) as f32;
    let height = source_height.max(1) as f32;
    let image_min = source_width.min(source_height).max(1) as f32;
    let radius_x = brush_size.clamp(f32::EPSILON, 0.5) * image_min / width;
    let radius_y = brush_size.clamp(f32::EPSILON, 0.5) * image_min / height;
    let gap_x = (radius_x * 0.85).max(6.0 / width);
    let gap_y = (radius_y * 0.85).max(6.0 / height);
    let center = [
        (focus.min[0] + focus.max[0]) * 0.5,
        (focus.min[1] + focus.max[1]) * 0.5,
    ];
    let guards = [
        [focus.min[0] - gap_x, focus.min[1] - gap_y],
        [center[0], focus.min[1] - gap_y],
        [focus.max[0] + gap_x, focus.min[1] - gap_y],
        [focus.min[0] - gap_x, center[1]],
        [focus.max[0] + gap_x, center[1]],
        [focus.min[0] - gap_x, focus.max[1] + gap_y],
        [center[0], focus.max[1] + gap_y],
        [focus.max[0] + gap_x, focus.max[1] + gap_y],
    ];
    for guard in guards {
        if prompts.len() >= limit {
            break;
        }
        let point = [guard[0].clamp(0.0, 1.0), guard[1].clamp(0.0, 1.0)];
        if !focus.contains(point) {
            prompts.push(ObjectPrompt {
                point,
                kind: ObjectPromptKind::Background,
            });
        }
    }

    ObjectPromptSet { prompts, focus }
}

fn object_prompt_focus(
    foreground_points: &[[f32; 2]],
    brush_size: f32,
    source_width: u32,
    source_height: u32,
) -> ObjectPromptFocus {
    let mut min = [1.0f32, 1.0f32];
    let mut max = [0.0f32, 0.0f32];
    for point in foreground_points {
        min[0] = min[0].min(point[0]);
        min[1] = min[1].min(point[1]);
        max[0] = max[0].max(point[0]);
        max[1] = max[1].max(point[1]);
    }
    if foreground_points.is_empty() {
        min = [0.45, 0.45];
        max = [0.55, 0.55];
    }

    let width = source_width.max(1) as f32;
    let height = source_height.max(1) as f32;
    let image_min = source_width.min(source_height).max(1) as f32;
    let radius = brush_size.clamp(f32::EPSILON, 0.5) * image_min;
    let padding_x = (radius * 1.35 + 8.0) / width;
    let padding_y = (radius * 1.35 + 8.0) / height;
    ObjectPromptFocus {
        min: [
            (min[0] - padding_x).clamp(0.0, 1.0),
            (min[1] - padding_y).clamp(0.0, 1.0),
        ],
        max: [
            (max[0] + padding_x).clamp(0.0, 1.0),
            (max[1] + padding_y).clamp(0.0, 1.0),
        ],
    }
}

fn evenly_sample<T: Copy>(values: &[T], count: usize) -> Vec<T> {
    if count == 0 || values.is_empty() {
        return Vec::new();
    }
    if values.len() <= count {
        return values.to_vec();
    }
    (0..count)
        .map(|index| values[index * (values.len() - 1) / (count - 1).max(1)])
        .collect()
}

pub(super) fn object_crop_for_prompts(
    width: u32,
    height: u32,
    focus: ObjectPromptFocus,
    expansion: usize,
) -> ObjectCropRect {
    if expansion >= 2 {
        return ObjectCropRect {
            x: 0,
            y: 0,
            width,
            height,
        };
    }
    let center_x = ((focus.min[0] + focus.max[0]) * 0.5 * width as f32).clamp(0.0, width as f32);
    let center_y = ((focus.min[1] + focus.max[1]) * 0.5 * height as f32).clamp(0.0, height as f32);
    let bounds_w = (focus.max[0] - focus.min[0]).max(0.0) * width as f32;
    let bounds_h = (focus.max[1] - focus.min[1]).max(0.0) * height as f32;
    let minimum = width.min(height) as f32 * 0.16;
    let factor = if expansion == 0 { 1.5 } else { 2.3 };
    let mut edge = bounds_w.max(bounds_h).max(minimum).max(96.0) * factor;
    edge = edge.min(width.max(height) as f32);
    let crop_width = edge.round().clamp(1.0, width as f32) as u32;
    let crop_height = edge.round().clamp(1.0, height as f32) as u32;
    let x = (center_x - crop_width as f32 * 0.5)
        .round()
        .clamp(0.0, width.saturating_sub(crop_width) as f32) as u32;
    let y = (center_y - crop_height as f32 * 0.5)
        .round()
        .clamp(0.0, height.saturating_sub(crop_height) as f32) as u32;
    ObjectCropRect {
        x,
        y,
        width: crop_width,
        height: crop_height,
    }
}

pub(super) fn crop_is_full(crop: ObjectCropRect, width: u32, height: u32) -> bool {
    crop.x == 0 && crop.y == 0 && crop.width == width && crop.height == height
}
