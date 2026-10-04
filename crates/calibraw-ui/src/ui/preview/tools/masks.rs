use super::super::*;
use super::brush::{
    sample_brush_stroke, OBJECT_BRUSH_MINIMUM_SPACING_FRACTION,
    STANDARD_BRUSH_MINIMUM_SPACING_FRACTION,
};
use crate::app::{BrushStrokeSamples, MaskPointerEdit, MaskToolAction};
use crate::pipeline::{MaskComponent, MaskStack};

mod overlay;

/// The mask state and view settings the mask tool reads each frame.
pub(in crate::ui::preview) struct MaskToolInput<'a> {
    stack: &'a MaskStack,
    drag: Option<MaskDragState>,
    last_brush_point: Option<[f32; 2]>,
    subject_refinement_active: bool,
    geometry: GeometryTransform,
    lens_geometry: Option<&'a LensGeometryMap>,
    zoom: f32,
    image_relative_brush_size: bool,
}

impl<'a> MaskToolInput<'a> {
    pub(in crate::ui::preview) fn of(app: &'a CalibRawApp) -> Self {
        Self {
            stack: &app.masks.stack,
            drag: app.masks.drag,
            last_brush_point: app.masks.last_brush_point,
            subject_refinement_active: app.masks.subject_refinement_active,
            geometry: app.develop.geometry,
            lens_geometry: loaded_lens_geometry(app).map(Arc::as_ref),
            zoom: app.preview.zoom,
            image_relative_brush_size: app.preferences.image_relative_brush_size,
        }
    }

    fn selected_component(&self) -> Option<(usize, usize, &'a MaskComponent)> {
        let mask_index = self.stack.selected_mask?;
        let component_index = self.stack.selected_component?;
        let component = self
            .stack
            .masks
            .get(mask_index)?
            .components
            .get(component_index)?;
        Some((mask_index, component_index, component))
    }
}

/// Where the pointer acts on the selected component this frame.
enum MaskPointerSample {
    Stroke(BrushStrokeSamples),
    Point([f32; 2]),
    /// A brush left the editable image; its stroke ends.
    StrokeLeftImage,
    /// A point tool outside the editable image is ignored.
    OutsideImage,
}

impl Preview {
    /// Read the pointer over the canvas and return the edits of the selected
    /// mask component, in the order they apply.
    pub(in crate::ui::preview) fn mask_tool_actions(
        ui: &Ui,
        input: &MaskToolInput<'_>,
        layout: PreviewLayout,
        response: &egui::Response,
    ) -> Vec<MaskToolAction> {
        let Some((mask_index, component_index, component)) = input.selected_component() else {
            return vec![MaskToolAction::Deactivate];
        };
        let kind = component.kind;
        if !kind.is_available() {
            return Vec::new();
        }
        if kind == MaskKind::Fullscreen {
            return vec![MaskToolAction::Deactivate];
        }
        let mut actions = vec![MaskToolAction::Activate(kind)];
        let subject_refining = input.subject_refinement_active
            && matches!(kind, MaskKind::Subject | MaskKind::Background);
        let geometry_can_leave_image =
            matches!(kind, MaskKind::Radial | MaskKind::Linear | MaskKind::Path)
                && (input.drag.is_some() || component.geometry.is_initialized());
        let pointer_bounds = if geometry_can_leave_image {
            layout.viewport_rect
        } else {
            layout.visible_rect
        };
        let pointer = response
            .interact_pointer_pos()
            .filter(|position| pointer_bounds.contains(*position));
        let (primary_is_down, primary_released) = ui.input(|input| {
            (
                input.pointer.primary_down(),
                input.pointer.primary_released(),
            )
        });
        let primary_down =
            pointer.is_some() && response.is_pointer_button_down_on() && primary_is_down;
        let Some(pointer) = pointer.filter(|_| primary_down) else {
            if primary_is_down {
                if subject_refining || matches!(kind, MaskKind::Brush | MaskKind::Object) {
                    actions.push(MaskToolAction::RestartStroke);
                }
            } else {
                let request_object = (primary_released
                    && !subject_refining
                    && object_strokes_await_selection(component))
                .then_some((mask_index, component_index));
                actions.push(MaskToolAction::EndGesture { request_object });
            }
            return actions;
        };
        if ui.input(|input| input.any_touches()) {
            actions.push(MaskToolAction::BeginTouchGesture {
                mask_index,
                component_index,
            });
        }

        // Parametric shapes live in the corrected image; image-derived masks
        // and brush strokes retain native coordinates so they follow the photo.
        let lens_geometry = input
            .lens_geometry
            .filter(|_| !matches!(kind, MaskKind::Radial | MaskKind::Linear));
        let projection = layout.projection(input.geometry, lens_geometry);
        let (uv, stroke) = match Self::sample_mask_pointer(
            input,
            projection,
            pointer,
            component,
            subject_refining,
            geometry_can_leave_image,
        ) {
            MaskPointerSample::Stroke(stroke) => (stroke.uv, Some(stroke)),
            MaskPointerSample::Point(uv) => (uv, None),
            MaskPointerSample::StrokeLeftImage => {
                actions.push(MaskToolAction::RestartStroke);
                return actions;
            }
            MaskPointerSample::OutsideImage => return actions,
        };

        if subject_refining {
            if let Some(stroke) = stroke {
                actions.push(MaskToolAction::PaintSubjectRefinement(stroke));
            }
            return actions;
        }
        if input.drag.is_none() && kind != MaskKind::Brush && kind != MaskKind::Object {
            let path_curve_modifier = ui.input(|input| input.modifiers.alt);
            actions.push(MaskToolAction::BeginDrag(begin_mask_drag(
                &component.geometry,
                uv,
                pointer,
                projection,
                path_curve_modifier,
            )));
        }
        actions.push(MaskToolAction::EditComponent(MaskPointerEdit {
            mask_index,
            component_index,
            kind,
            uv,
            stroke,
            source_width: layout.source_width,
            source_height: layout.source_height,
        }));
        actions
    }

    /// Brush tools sample dabs along the pointer path; other tools take the
    /// pointer position.
    fn sample_mask_pointer(
        input: &MaskToolInput<'_>,
        projection: SourceProjection<'_>,
        pointer: Pos2,
        component: &MaskComponent,
        subject_refining: bool,
        geometry_can_leave_image: bool,
    ) -> MaskPointerSample {
        let kind = component.kind;
        let brush_tool_size = if subject_refining {
            Some(input.stack.subject_refinement.size)
        } else {
            match (&component.geometry, kind) {
                (MaskGeometry::Brush { size, .. }, MaskKind::Brush) => Some(*size),
                (MaskGeometry::Object { brush_size, .. }, MaskKind::Object) => Some(*brush_size),
                _ => None,
            }
        };
        if let Some(tool_size) = brush_tool_size {
            let mut previous = input.last_brush_point;
            return sample_brush_stroke(
                projection,
                pointer,
                tool_size,
                input.zoom,
                input.image_relative_brush_size,
                &mut previous,
                if kind == MaskKind::Object {
                    OBJECT_BRUSH_MINIMUM_SPACING_FRACTION
                } else {
                    STANDARD_BRUSH_MINIMUM_SPACING_FRACTION
                },
            )
            .map_or(
                MaskPointerSample::StrokeLeftImage,
                MaskPointerSample::Stroke,
            );
        }
        let source_uv = projection.to_source(pointer);
        if geometry_can_leave_image {
            return MaskPointerSample::Point(source_uv);
        }
        editable_source_uv(source_uv)
            .map_or(MaskPointerSample::OutsideImage, MaskPointerSample::Point)
    }
}

/// An object component with painted strokes but no selection yet.
fn object_strokes_await_selection(component: &MaskComponent) -> bool {
    component.kind == MaskKind::Object
        && matches!(
            &component.geometry,
            MaskGeometry::Object { mask: None, strokes, .. }
                if strokes.iter().any(|stroke| !stroke.points.is_empty())
        )
}
