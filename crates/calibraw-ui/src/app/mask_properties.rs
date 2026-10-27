//! Edits from the mask properties panel (`ui::sidebar::masks::properties`).
//! The panel reads the selected mask and the tool settings and returns
//! `MaskPropertyAction`s; `apply_mask_property_actions` applies them to the
//! mask and to the tool settings the caller writes back afterwards.

use super::*;
use crate::pipeline::{DepthRangeSettings, LocalMask, MaskCombineMode, MaskComponent, PersonPart};
use std::collections::BTreeSet;

/// Mask-tool state edited by the properties panel. The caller loads it from
/// the app before drawing and applies it, with its requests, afterwards.
#[derive(Clone, Copy)]
pub(crate) struct MaskPropertiesControls {
    pub(crate) brush_mode: BrushMode,
    /// False while a subject, sky, or depth model is already running.
    pub(crate) generation_idle: bool,
    pub(crate) refinement_active: bool,
    pub(crate) refinement_size: f32,
    pub(crate) refinement_feather: f32,
    pub(crate) refinement_flow: f32,
    pub(crate) clear_refinement: bool,
    /// Generate (or regenerate) the subject, sky, or depth map of the component.
    pub(crate) request_generation: bool,
    pub(crate) request_object: bool,
    /// Make the people mask again, for its person and parts.
    pub(crate) request_people: bool,
    pub(crate) people_edit: Option<PeopleEdit>,
    pub(crate) separate_person_parts: bool,
    /// The person whose chip the pointer is over.
    pub(crate) hovered_person: Option<usize>,
}

/// A people mask's people and parts, for its properties.
#[derive(Default)]
pub(crate) struct PeopleChoices {
    /// Whether the people in the photo are known; until then the panel
    /// offers to find them.
    pub(crate) analysed: bool,
    /// Each person's thumbnail, left to right.
    pub(crate) people: Vec<PersonThumbnail>,
    /// The people the mask selects, in order.
    pub(crate) selected: Vec<usize>,
    pub(crate) parts: BTreeSet<PersonPart>,
    /// The parts any of the mask's people shows, once known.
    pub(crate) shown: Option<BTreeSet<PersonPart>>,
}

/// One person's chip in a people mask's properties.
pub(crate) struct PersonThumbnail {
    pub(crate) texture: egui::TextureId,
    /// Their body parts are known, so adding them runs no model.
    pub(crate) segmented: bool,
}

/// A change to a people mask's people or parts, which
/// `CalibRawApp::edit_people_mask` makes together with the mask it needs.
#[derive(Clone, Copy, Debug)]
pub(crate) enum PeopleEdit {
    /// Adds a person, by their index in the people found, or removes them;
    /// the last person stays.
    TogglePerson(usize),
    /// Adds or removes a part; the last part stays. The entire person
    /// replaces the other parts, which replace it in turn.
    TogglePart(PersonPart),
}

pub(crate) enum MaskPropertyAction {
    /// Whole-mask opacity.
    SetOpacity(f32),
    ToggleInvert,
    SetCombine(MaskCombineMode),
    SetBrushSize(f32),
    /// Also re-feathers the existing dabs.
    SetBrushFeather(f32),
    ToggleBrushOpacity,
    ToggleBrushOverlap,
    /// Opacity of newly drawn brush strokes.
    SetBrushStrokeOpacity(f32),
    ClearBrushStrokes,
    /// Edge feather of a shape, path, AI, object or range component.
    SetFeather(f32),
    SetGrow(f32),
    StraightenPath,
    RemoveLastPathPoint,
    ClearPath,
    SetObjectBrushSize(f32),
    SetEdgeRefine(f32),
    ClearObjectSelection,
    SetLuminanceLow(f32),
    SetLuminanceHigh(f32),
    /// Fade above the high bound; `SetFeather` sets the one below `low`.
    SetLuminanceHighFeather(f32),
    SetColorTolerance(f32),
    SetDepthRange(DepthRangeSettings),
    /// Adds a person to a people mask or removes them, as
    /// `PeopleEdit::TogglePerson`.
    TogglePerson(usize),
    /// Adds or removes a part of a people mask, as `PeopleEdit::TogglePart`.
    TogglePersonPart(PersonPart),
    SeparatePersonParts,
    RequestPeople,
    /// The pointer is over a person's chip.
    HoverPerson(usize),
    SetBrushMode(BrushMode),
    SetRefinementActive(bool),
    SetRefinementSize(f32),
    SetRefinementFeather(f32),
    SetRefinementFlow(f32),
    ClearRefinement,
    RequestGeneration,
    RequestObject,
}

/// Apply the panel's actions in order. Returns whether the mask coverage
/// changed.
pub(crate) fn apply_mask_property_actions(
    mask: &mut LocalMask,
    component_index: usize,
    controls: &mut MaskPropertiesControls,
    actions: Vec<MaskPropertyAction>,
) -> bool {
    let mut geometry_changed = false;
    for action in actions {
        geometry_changed |= apply_mask_property_action(mask, component_index, controls, action);
    }
    geometry_changed
}

fn apply_mask_property_action(
    mask: &mut LocalMask,
    component_index: usize,
    controls: &mut MaskPropertiesControls,
    action: MaskPropertyAction,
) -> bool {
    match action {
        MaskPropertyAction::SetOpacity(opacity) => {
            mask.set_opacity(opacity);
            true
        }
        MaskPropertyAction::SetBrushMode(mode) => {
            controls.brush_mode = mode;
            false
        }
        MaskPropertyAction::SetRefinementActive(active) => {
            controls.refinement_active = active;
            false
        }
        MaskPropertyAction::SetRefinementSize(size) => {
            controls.refinement_size = size;
            false
        }
        MaskPropertyAction::SetRefinementFeather(feather) => {
            controls.refinement_feather = feather;
            false
        }
        MaskPropertyAction::SetRefinementFlow(flow) => {
            controls.refinement_flow = flow;
            false
        }
        MaskPropertyAction::ClearRefinement => {
            controls.clear_refinement = true;
            false
        }
        MaskPropertyAction::RequestGeneration => {
            controls.request_generation = true;
            false
        }
        MaskPropertyAction::RequestObject => {
            controls.request_object = true;
            false
        }
        MaskPropertyAction::SeparatePersonParts => {
            controls.separate_person_parts = true;
            false
        }
        MaskPropertyAction::RequestPeople => {
            controls.request_people = true;
            false
        }
        MaskPropertyAction::HoverPerson(person) => {
            controls.hovered_person = Some(person);
            false
        }
        // The people and parts change with the mask they need.
        MaskPropertyAction::TogglePerson(person) => {
            controls.people_edit = Some(PeopleEdit::TogglePerson(person));
            false
        }
        MaskPropertyAction::TogglePersonPart(part) => {
            controls.people_edit = Some(PeopleEdit::TogglePart(part));
            false
        }
        component_action => mask
            .components
            .get_mut(component_index)
            .is_some_and(|component| apply_component_action(component, component_action)),
    }
}

fn apply_component_action(component: &mut MaskComponent, action: MaskPropertyAction) -> bool {
    match action {
        MaskPropertyAction::ToggleInvert => {
            component.common.toggle_invert();
            true
        }
        MaskPropertyAction::SetCombine(combine) => component.set_combine(combine),
        MaskPropertyAction::SetFeather(value) => {
            // A luminance range without its own high feather softens its
            // bright edge with `feather`. Keep that edge as it is before
            // `feather` becomes the dark edge's alone.
            if let MaskGeometry::LuminanceRange {
                feather,
                high_feather: high_feather @ None,
                ..
            } = &mut component.geometry
            {
                *high_feather = Some(*feather);
            }
            shape_feather(&mut component.geometry)
                .map(|feather| *feather = value)
                .is_some()
        }
        MaskPropertyAction::SetGrow(value) => grow(&mut component.geometry)
            .map(|grow| *grow = value)
            .is_some(),
        action => apply_geometry_action(&mut component.geometry, action),
    }
}

/// The edge feather of every geometry except brushes, whose feather also
/// applies to existing dabs.
fn shape_feather(geometry: &mut MaskGeometry) -> Option<&mut f32> {
    match geometry {
        MaskGeometry::Radial { feather, .. }
        | MaskGeometry::Linear { feather, .. }
        | MaskGeometry::Path { feather, .. }
        | MaskGeometry::Ai { feather, .. }
        | MaskGeometry::Object { feather, .. }
        | MaskGeometry::Person { feather, .. }
        | MaskGeometry::LuminanceRange { feather, .. } => Some(feather),
        _ => None,
    }
}

fn grow(geometry: &mut MaskGeometry) -> Option<&mut f32> {
    match geometry {
        MaskGeometry::Path { grow, .. }
        | MaskGeometry::Ai { grow, .. }
        | MaskGeometry::Object { grow, .. }
        | MaskGeometry::Person { grow, .. }
        | MaskGeometry::LuminanceRange { grow, .. }
        | MaskGeometry::ColorRange { grow, .. } => Some(grow),
        _ => None,
    }
}

/// Returns whether the coverage changed; actions for another geometry do nothing.
fn apply_geometry_action(geometry: &mut MaskGeometry, action: MaskPropertyAction) -> bool {
    match (geometry, action) {
        (MaskGeometry::Brush { size, .. }, MaskPropertyAction::SetBrushSize(value)) => {
            *size = value;
            true
        }
        (MaskGeometry::Brush { feather, dabs, .. }, MaskPropertyAction::SetBrushFeather(value)) => {
            *feather = value;
            for dab in dabs.iter_mut() {
                dab.feather = *feather;
            }
            true
        }
        (
            MaskGeometry::Brush {
                opacity_enabled, ..
            },
            MaskPropertyAction::ToggleBrushOpacity,
        ) => {
            *opacity_enabled = !*opacity_enabled;
            true
        }
        (
            MaskGeometry::Brush {
                overlap_enabled, ..
            },
            MaskPropertyAction::ToggleBrushOverlap,
        ) => {
            *overlap_enabled = !*overlap_enabled;
            true
        }
        (MaskGeometry::Brush { opacity, .. }, MaskPropertyAction::SetBrushStrokeOpacity(value)) => {
            *opacity = value;
            true
        }
        (
            MaskGeometry::Brush {
                dabs,
                stroke_starts,
                ..
            },
            MaskPropertyAction::ClearBrushStrokes,
        ) => {
            dabs.clear();
            stroke_starts.clear();
            true
        }
        (MaskGeometry::Path { points, .. }, MaskPropertyAction::StraightenPath) => {
            for point in points.iter_mut() {
                point.handle_in = [0.0, 0.0];
                point.handle_out = [0.0, 0.0];
            }
            true
        }
        (MaskGeometry::Path { points, .. }, MaskPropertyAction::RemoveLastPathPoint) => {
            points.pop().is_some()
        }
        (MaskGeometry::Path { points, .. }, MaskPropertyAction::ClearPath) => {
            points.clear();
            true
        }
        (
            MaskGeometry::Object { brush_size, .. },
            MaskPropertyAction::SetObjectBrushSize(value),
        ) => {
            *brush_size = value;
            true
        }
        (MaskGeometry::Object { edge_refine, .. }, MaskPropertyAction::SetEdgeRefine(value)) => {
            *edge_refine = value;
            true
        }
        (MaskGeometry::Object { mask, strokes, .. }, MaskPropertyAction::ClearObjectSelection) => {
            strokes.clear();
            *mask = None;
            true
        }
        (MaskGeometry::LuminanceRange { low, .. }, MaskPropertyAction::SetLuminanceLow(value)) => {
            *low = value;
            true
        }
        (
            MaskGeometry::LuminanceRange { high, .. },
            MaskPropertyAction::SetLuminanceHigh(value),
        ) => {
            *high = value;
            true
        }
        (
            MaskGeometry::LuminanceRange { high_feather, .. },
            MaskPropertyAction::SetLuminanceHighFeather(value),
        ) => {
            *high_feather = Some(value);
            true
        }
        (
            MaskGeometry::ColorRange { tolerance, .. },
            MaskPropertyAction::SetColorTolerance(value),
        ) => {
            *tolerance = value;
            true
        }
        (MaskGeometry::DepthRange { range, .. }, MaskPropertyAction::SetDepthRange(value)) => {
            *range = value;
            true
        }
        _ => false,
    }
}
