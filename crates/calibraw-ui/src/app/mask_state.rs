//! Mask editing state: drags, touch gestures, overlays and AI mask targets.

use super::*;

#[derive(Clone, Copy, Debug)]
pub(crate) enum MaskDragState {
    Create([f32; 2]),
    MoveRadial {
        pointer: [f32; 2],
        center: [f32; 2],
    },
    ResizeRadial {
        axis: usize,
    },
    RotateRadial {
        pointer_angle: f32,
        rotation: f32,
    },
    MoveLinear {
        pointer: [f32; 2],
        start: [f32; 2],
        end: [f32; 2],
    },
    LinearStart,
    LinearEnd,
    RotateLinear {
        pointer_angle: f32,
        start: [f32; 2],
        end: [f32; 2],
    },
    AddPathPoint {
        index: usize,
        anchor: [f32; 2],
    },
    MovePathPoint {
        index: usize,
    },
    MovePathHandle {
        index: usize,
        outgoing: bool,
    },
    CreatePathHandles {
        index: usize,
    },
}

#[derive(Clone)]
pub(crate) struct MaskTouchGestureBackup {
    pub(super) mask_index: usize,
    pub(super) component_index: usize,
    pub(super) geometry: MaskGeometry,
    pub(super) subject_refinement: Option<SubjectRefinement>,
    pub(super) object_cache: Option<((usize, usize), ObjectInferenceCache)>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum MaskOverlayBlink {
    #[default]
    GroupTwice,
    ComponentThenGroup,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct AiMaskTarget {
    pub(super) mask_index: usize,
    pub(super) component_index: usize,
    pub(super) kind: MaskKind,
    pub(super) geometry: MaskGeometry,
}

pub(crate) struct MaskState {
    pub(crate) stack: MaskStack,
    pub(crate) active_tool: Option<MaskKind>,
    pub(crate) brush_mode: BrushMode,
    pub(crate) subject_refinement_active: bool,
    pub(crate) drag: Option<MaskDragState>,
    pub(crate) last_brush_point: Option<[f32; 2]>,
    pub(crate) touch_gesture_backup: Option<MaskTouchGestureBackup>,
    pub(crate) interaction_dirty_layer: Option<usize>,
    pub(crate) interaction_last_upload: Option<Instant>,
    pub(crate) interaction_has_uncommitted_change: bool,
    pub(crate) overlay_revision: u64,
    pub(crate) overlay_texture: Option<egui::TextureHandle>,
    pub(crate) overlay_texture_key: Option<(usize, Option<usize>, u64, OverlayRasterKey)>,
    pub(crate) overlay_blink: Option<(std::time::Instant, MaskOverlayBlink)>,
    pub(crate) thumbnail_revision: u64,
    pub(crate) thumbnail_group_textures: Vec<egui::TextureHandle>,
    pub(crate) thumbnail_component_mask: Option<usize>,
    pub(crate) thumbnail_component_textures: Vec<egui::TextureHandle>,
    pub(crate) source_cache: Option<MaskRgbImage>,
    pub(crate) subject_cache: Option<MaskImage>,
    pub(crate) sky_cache: Option<MaskImage>,
    pub(crate) depth_cache: Option<MaskImage>,
    pub(crate) dirty_layers: [bool; MAX_LOCAL_MASKS],
    pub(crate) detail_dirty_layers: [bool; MAX_LOCAL_MASKS],
    pub(crate) navigation_dirty_layers: [bool; MAX_LOCAL_MASKS],
}
