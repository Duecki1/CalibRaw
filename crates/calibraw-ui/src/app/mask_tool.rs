//! Pointer edits of the selected mask component. The preview's mask tool
//! (`ui::preview::tools::masks`) reads the pointer and returns these actions;
//! `apply_mask_tool_actions` applies them in order within the same frame.

use super::*;
use crate::pipeline::{BrushDab, ObjectStroke, PathPoint, MAX_PATH_POINTS};
use crate::ui::preview::{shortest_angle_delta, source_angle_from};
use calibraw_core::color_math::{srgb_decode, srgb_encode};

/// Upper bound on subject-refinement dabs shared by Subject and Background.
const MAX_REFINEMENT_DABS: usize = 65_536;
/// Upper bound on dabs in one brush component and on points in one object stroke.
const MAX_STROKE_POINTS: usize = 8192;

/// Brush dabs sampled along the pointer path since the previous frame.
/// Positions are normalized source coordinates.
pub(crate) struct BrushStrokeSamples {
    /// The pointer position this frame.
    pub(crate) uv: [f32; 2],
    /// Dab size after zoom compensation, in the units of the brush size.
    pub(crate) dab_size: f32,
    /// True when this sample starts a new stroke.
    pub(crate) first: bool,
    pub(crate) samples: Vec<[f32; 2]>,
}

pub(crate) enum MaskToolAction {
    /// Nothing editable is selected: end the interaction and drop the tool.
    Deactivate,
    Activate(MaskKind),
    /// The next brush sample starts a new stroke.
    RestartStroke,
    /// The primary button is up: end the drag and keep the touch edit. An
    /// object component whose strokes are not yet selected requests its mask.
    EndGesture {
        request_object: Option<(usize, usize)>,
    },
    /// Back up the component so a second finger can cancel the edit.
    BeginTouchGesture {
        mask_index: usize,
        component_index: usize,
    },
    BeginDrag(Option<MaskDragState>),
    PaintSubjectRefinement(BrushStrokeSamples),
    EditComponent(MaskPointerEdit),
}

/// The pointer acting on one component, in normalized source coordinates.
pub(crate) struct MaskPointerEdit {
    pub(crate) mask_index: usize,
    pub(crate) component_index: usize,
    pub(crate) kind: MaskKind,
    pub(crate) uv: [f32; 2],
    /// Present for brush and object components.
    pub(crate) stroke: Option<BrushStrokeSamples>,
    /// Source pixel dimensions; handle rotation and resizing work in pixels.
    pub(crate) source_width: u32,
    pub(crate) source_height: u32,
}

impl CalibRawApp {
    pub(crate) fn apply_mask_tool_actions(
        &mut self,
        ctx: &egui::Context,
        actions: Vec<MaskToolAction>,
    ) {
        for action in actions {
            self.apply_mask_tool_action(ctx, action);
        }
    }

    fn apply_mask_tool_action(&mut self, ctx: &egui::Context, action: MaskToolAction) {
        match action {
            MaskToolAction::Deactivate => {
                self.finish_mask_geometry_interaction();
                self.masks.active_tool = None;
            }
            MaskToolAction::Activate(kind) => self.masks.active_tool = Some(kind),
            MaskToolAction::RestartStroke => self.masks.last_brush_point = None,
            MaskToolAction::EndGesture { request_object } => {
                self.finish_mask_geometry_interaction();
                self.masks.last_brush_point = None;
                self.masks.drag = None;
                self.commit_mask_touch_gesture();
                if let Some((mask_index, component_index)) = request_object {
                    self.request_object_mask(mask_index, component_index);
                }
            }
            MaskToolAction::BeginTouchGesture {
                mask_index,
                component_index,
            } => self.begin_mask_touch_gesture(mask_index, component_index),
            MaskToolAction::BeginDrag(drag) => self.masks.drag = drag,
            MaskToolAction::PaintSubjectRefinement(stroke) => {
                self.paint_subject_refinement(ctx, &stroke);
            }
            MaskToolAction::EditComponent(edit) => self.edit_mask_component(ctx, edit),
        }
    }

    fn paint_subject_refinement(&mut self, ctx: &egui::Context, stroke: &BrushStrokeSamples) {
        let refinement = &mut self.masks.stack.subject_refinement;
        let opacity = self.masks.brush_mode.dab_opacity(true, refinement.flow);
        let mut changed = false;
        if stroke.first && !stroke.samples.is_empty() && refinement.dabs.len() < MAX_REFINEMENT_DABS
        {
            refinement.stroke_starts.push(refinement.dabs.len());
        }
        for &center in &stroke.samples {
            if refinement.dabs.len() >= MAX_REFINEMENT_DABS {
                break;
            }
            refinement.dabs.push(BrushDab {
                center,
                opacity,
                size: stroke.dab_size,
                feather: refinement.feather,
            });
            changed = true;
        }
        if changed {
            self.masks.last_brush_point = Some(stroke.uv);
            self.note_subject_refinement_interaction();
            ctx.request_repaint();
        }
    }

    fn edit_mask_component(&mut self, ctx: &egui::Context, edit: MaskPointerEdit) {
        let MaskPointerEdit {
            mask_index,
            mut component_index,
            kind,
            uv,
            stroke,
            source_width,
            source_height,
        } = edit;
        let color_was_sampled = self
            .masks
            .stack
            .masks
            .get(mask_index)
            .and_then(|mask| mask.components.get(component_index))
            .is_some_and(|component| {
                matches!(
                    &component.geometry,
                    MaskGeometry::ColorRange { sampled: true, .. }
                )
            });

        let mut changed = false;
        if kind == MaskKind::Object && self.masks.last_brush_point.is_none() {
            let Some(target) = self.prepare_object_mask_for_stroke(mask_index, component_index)
            else {
                return;
            };
            changed |= target != component_index;
            component_index = target;
        }

        let brush_mode = self.masks.brush_mode;
        let drag = self.masks.drag;
        let source = (source_width, source_height);
        if let Some(component) = self
            .masks
            .stack
            .masks
            .get_mut(mask_index)
            .and_then(|mask| mask.components.get_mut(component_index))
        {
            let edited = match (&mut component.geometry, kind) {
                (geometry @ MaskGeometry::Brush { .. }, MaskKind::Brush) => {
                    let Some(stroke) = stroke.as_ref() else {
                        return;
                    };
                    let painted = paint_brush_dabs(geometry, brush_mode, stroke);
                    if painted {
                        self.masks.last_brush_point = Some(stroke.uv);
                    }
                    painted
                }
                (geometry @ MaskGeometry::Radial { .. }, MaskKind::Radial) => {
                    drag_radial(geometry, drag, uv, source)
                }
                (geometry @ MaskGeometry::Linear { .. }, MaskKind::Linear) => {
                    drag_linear(geometry, drag, uv, source)
                }
                (MaskGeometry::Path { points, .. }, MaskKind::Path) => {
                    drag_path(points, drag, uv, source)
                }
                (MaskGeometry::Object { strokes, .. }, MaskKind::Object) => {
                    let Some(stroke) = stroke.as_ref() else {
                        return;
                    };
                    changed |= extend_object_strokes(strokes, stroke);
                    if changed {
                        self.masks.last_brush_point = Some(stroke.uv);
                    }
                    false
                }
                (
                    geometry @ MaskGeometry::ColorRange {
                        source: Some(_), ..
                    },
                    MaskKind::ColorRange,
                ) => sample_color_range(geometry, uv),
                _ => false,
            };
            changed |= edited;
        }

        if changed {
            self.note_mask_geometry_interaction(mask_index);
            if kind == MaskKind::ColorRange && !color_was_sampled {
                self.blink_selected_component();
            }
            ctx.request_repaint();
        }
    }
}

fn paint_brush_dabs(
    geometry: &mut MaskGeometry,
    brush_mode: BrushMode,
    stroke: &BrushStrokeSamples,
) -> bool {
    let MaskGeometry::Brush {
        feather,
        opacity_enabled,
        opacity: brush_opacity,
        stroke_starts,
        dabs,
        ..
    } = geometry
    else {
        return false;
    };
    let opacity = brush_mode.dab_opacity(*opacity_enabled, *brush_opacity);
    if stroke.first && !stroke.samples.is_empty() && dabs.len() < MAX_STROKE_POINTS {
        stroke_starts.push(dabs.len());
    }
    let mut changed = false;
    for &center in &stroke.samples {
        if dabs.len() >= MAX_STROKE_POINTS {
            break;
        }
        dabs.push(BrushDab {
            center,
            opacity,
            size: stroke.dab_size,
            feather: *feather,
        });
        changed = true;
    }
    changed
}

fn drag_radial(
    geometry: &mut MaskGeometry,
    drag: Option<MaskDragState>,
    uv: [f32; 2],
    (source_width, source_height): (u32, u32),
) -> bool {
    let MaskGeometry::Radial {
        center,
        radius,
        rotation,
        initialized,
        ..
    } = geometry
    else {
        return false;
    };
    match drag {
        Some(MaskDragState::Create(origin)) => {
            let mut rx = (uv[0] - origin[0]).abs();
            let mut ry = (uv[1] - origin[1]).abs();
            if rx < 0.01 && ry >= 0.01 {
                rx = ry * 0.66;
            }
            if ry < 0.01 && rx >= 0.01 {
                ry = rx * 0.66;
            }
            *center = origin;
            *radius = [rx.max(0.005), ry.max(0.005)];
            *rotation = 0.0;
            *initialized = rx > 0.008 || ry > 0.008;
            true
        }
        Some(MaskDragState::MoveRadial {
            pointer: origin,
            center: original_center,
        }) => {
            center[0] = original_center[0] + uv[0] - origin[0];
            center[1] = original_center[1] + uv[1] - origin[1];
            true
        }
        Some(MaskDragState::ResizeRadial { axis }) => {
            let dx = (uv[0] - center[0]) * source_width.max(1) as f32;
            let dy = (uv[1] - center[1]) * source_height.max(1) as f32;
            let cos_r = rotation.cos();
            let sin_r = rotation.sin();
            if axis == 0 {
                radius[0] =
                    ((cos_r * dx + sin_r * dy).abs() / source_width.max(1) as f32).max(0.005);
            } else {
                radius[1] =
                    ((-sin_r * dx + cos_r * dy).abs() / source_height.max(1) as f32).max(0.005);
            }
            true
        }
        Some(MaskDragState::RotateRadial {
            pointer_angle,
            rotation: original_rotation,
        }) => {
            let current_angle = source_angle_from(*center, uv, source_width, source_height);
            *rotation = original_rotation + shortest_angle_delta(pointer_angle, current_angle);
            true
        }
        _ => false,
    }
}

fn drag_linear(
    geometry: &mut MaskGeometry,
    drag: Option<MaskDragState>,
    uv: [f32; 2],
    (source_width, source_height): (u32, u32),
) -> bool {
    let MaskGeometry::Linear {
        start,
        end,
        initialized,
        ..
    } = geometry
    else {
        return false;
    };
    match drag {
        Some(MaskDragState::Create(origin)) => {
            *start = origin;
            *end = uv;
            let dx = end[0] - start[0];
            let dy = end[1] - start[1];
            *initialized = dx * dx + dy * dy > 0.000_025;
            true
        }
        Some(MaskDragState::LinearStart) => {
            *start = uv;
            true
        }
        Some(MaskDragState::LinearEnd) => {
            *end = uv;
            true
        }
        Some(MaskDragState::MoveLinear {
            pointer: origin,
            start: original_start,
            end: original_end,
        }) => {
            let dx = uv[0] - origin[0];
            let dy = uv[1] - origin[1];
            *start = [original_start[0] + dx, original_start[1] + dy];
            *end = [original_end[0] + dx, original_end[1] + dy];
            true
        }
        Some(MaskDragState::RotateLinear {
            pointer_angle,
            start: original_start,
            end: original_end,
        }) => {
            let midpoint = [
                (original_start[0] + original_end[0]) * 0.5,
                (original_start[1] + original_end[1]) * 0.5,
            ];
            let vector_x = (original_end[0] - original_start[0]) * source_width.max(1) as f32;
            let vector_y = (original_end[1] - original_start[1]) * source_height.max(1) as f32;
            let original_angle = vector_y.atan2(vector_x);
            let current_angle = source_angle_from(midpoint, uv, source_width, source_height);
            let angle = original_angle + shortest_angle_delta(pointer_angle, current_angle);
            let half_length = (vector_x * vector_x + vector_y * vector_y).sqrt() * 0.5;
            let half_x = angle.cos() * half_length;
            let half_y = angle.sin() * half_length;
            *start = [
                midpoint[0] - half_x / source_width.max(1) as f32,
                midpoint[1] - half_y / source_height.max(1) as f32,
            ];
            *end = [
                midpoint[0] + half_x / source_width.max(1) as f32,
                midpoint[1] + half_y / source_height.max(1) as f32,
            ];
            true
        }
        _ => false,
    }
}

fn drag_path(
    points: &mut Vec<PathPoint>,
    drag: Option<MaskDragState>,
    uv: [f32; 2],
    (source_width, source_height): (u32, u32),
) -> bool {
    let mut changed = false;
    match drag {
        Some(MaskDragState::AddPathPoint { index, anchor }) => {
            if index == points.len() && points.len() < MAX_PATH_POINTS {
                points.push(PathPoint::corner(anchor));
                changed = true;
            }
            if let Some(point) = points.get_mut(index) {
                let dx = uv[0] - anchor[0];
                let dy = uv[1] - anchor[1];
                let px = dx * source_width.max(1) as f32;
                let py = dy * source_height.max(1) as f32;
                if px * px + py * py >= 9.0 {
                    changed |= set_symmetric_handles(point, [dx, dy]);
                }
            }
        }
        Some(MaskDragState::MovePathPoint { index }) => {
            if let Some(point) = points.get_mut(index) {
                if point.position != uv {
                    point.position = uv;
                    changed = true;
                }
            }
        }
        Some(MaskDragState::MovePathHandle { index, outgoing }) => {
            if let Some(point) = points.get_mut(index) {
                let offset = [uv[0] - point.position[0], uv[1] - point.position[1]];
                let target = if outgoing {
                    &mut point.handle_out
                } else {
                    &mut point.handle_in
                };
                if *target != offset {
                    *target = offset;
                    changed = true;
                }
            }
        }
        Some(MaskDragState::CreatePathHandles { index }) => {
            if let Some(point) = points.get_mut(index) {
                let offset = [uv[0] - point.position[0], uv[1] - point.position[1]];
                changed |= set_symmetric_handles(point, offset);
            }
        }
        _ => {}
    }
    changed
}

/// Mirror the incoming handle of a smooth point about its anchor.
fn set_symmetric_handles(point: &mut PathPoint, outgoing: [f32; 2]) -> bool {
    let incoming = [-outgoing[0], -outgoing[1]];
    if point.handle_in == incoming && point.handle_out == outgoing {
        return false;
    }
    point.handle_in = incoming;
    point.handle_out = outgoing;
    true
}

fn extend_object_strokes(strokes: &mut Vec<ObjectStroke>, stroke: &BrushStrokeSamples) -> bool {
    if stroke.first {
        let Some(&first) = stroke.samples.first() else {
            return false;
        };
        strokes.push(ObjectStroke {
            points: vec![first],
            positive: true,
            brush_size: stroke.dab_size,
        });
        return true;
    }
    let Some(object_stroke) = strokes.last_mut() else {
        return false;
    };
    let before = object_stroke.points.len();
    for &point in &stroke.samples {
        if object_stroke.points.len() >= MAX_STROKE_POINTS {
            break;
        }
        object_stroke.points.push(point);
    }
    object_stroke.points.len() != before
}

/// Source pixels on each side of the sampled one that its colour is averaged
/// over, so sensor noise does not decide the sampled colour.
const COLOR_SAMPLE_RADIUS: usize = 2;

/// Sample the source colour around `uv`; `uv` lies inside the editable image.
/// Averages in linear light and stores display-encoded sRGB.
fn sample_color_range(geometry: &mut MaskGeometry, uv: [f32; 2]) -> bool {
    let MaskGeometry::ColorRange {
        source: Some(source),
        sample,
        sampled,
        ..
    } = geometry
    else {
        return false;
    };
    let width = source.width as usize;
    let height = source.height as usize;
    if width == 0 || height == 0 {
        return false;
    }
    let x = (uv[0] * width.saturating_sub(1) as f32).round() as usize;
    let y = (uv[1] * height.saturating_sub(1) as f32).round() as usize;
    let columns = x.saturating_sub(COLOR_SAMPLE_RADIUS)..=(x + COLOR_SAMPLE_RADIUS).min(width - 1);
    let rows = y.saturating_sub(COLOR_SAMPLE_RADIUS)..=(y + COLOR_SAMPLE_RADIUS).min(height - 1);
    let mut sum = [0.0f32; 3];
    let mut count = 0.0f32;
    for row in rows {
        for column in columns.clone() {
            let index = (row * width + column) * 4;
            for (channel, total) in sum.iter_mut().enumerate() {
                *total += srgb_decode(source.rgba[index + channel] as f32 / 255.0);
            }
            count += 1.0;
        }
    }
    *sample = sum.map(|total| srgb_encode(total / count));
    *sampled = true;
    true
}
