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

/// What a coverage overlay shows: the selected mask's sub-mask, or the whole
/// group (`component_index: None`), at a mask revision, over a `region` of a
/// source of `full_size` pixels at a texture size, in lens-corrected space or
/// not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CoverageKey {
    pub(crate) mask_index: usize,
    pub(crate) component_index: Option<usize>,
    pub(crate) revision: u64,
    pub(crate) region: OverlayRasterKey,
    pub(crate) full_size: [u32; 2],
    pub(crate) corrected_space: bool,
}

impl CoverageKey {
    /// Whether `other` shows the same coverage, perhaps of an older revision
    /// or another region.
    pub(crate) fn same_target(self, other: Self) -> bool {
        self.mask_index == other.mask_index && self.component_index == other.component_index
    }
}

/// The selected mask's coverage overlay texture and the draw that replaces
/// it.
///
/// Drawing the coverage at the canvas's physical resolution takes a phone
/// CPU long enough to stall the frame, so the costly overlays are drawn on a
/// worker (`request`) and the last texture stays on screen until it lands;
/// quick ones are drawn in the frame (`set`). One draw is
/// in flight at a time: a key that changes meanwhile (a pan, a zoom) is
/// drawn once the current draw finishes, so a gesture never queues work.
#[derive(Default)]
pub(crate) struct CoverageOverlay {
    texture: Option<(egui::TextureHandle, CoverageKey)>,
    job: Option<CoverageJob>,
}

struct CoverageJob {
    key: CoverageKey,
    receiver: mpsc::Receiver<egui::ColorImage>,
}

impl CoverageOverlay {
    /// Drops the texture and forgets a draw in flight, whose result is then
    /// discarded.
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// The texture on screen and what it shows.
    pub(crate) fn texture(&self) -> Option<(egui::TextureId, CoverageKey)> {
        self.texture
            .as_ref()
            .map(|(texture, key)| (texture.id(), *key))
    }

    /// Installs a finished draw, if one has landed.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let Some(job) = self.job.as_ref() else {
            return;
        };
        match job.receiver.try_recv() {
            Ok(image) => {
                let key = job.key;
                self.job = None;
                self.set(ctx, key, image);
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => self.job = None,
        }
    }

    /// Whether the texture shows `key` or a draw of it is in flight.
    pub(crate) fn has(&self, key: CoverageKey) -> bool {
        self.texture
            .as_ref()
            .is_some_and(|(_, shown)| *shown == key)
            || self.job.as_ref().is_some_and(|job| job.key == key)
    }

    /// Shows `image`, drawn for `key`, now. A draw in flight is older and is
    /// forgotten, so it cannot replace this one when it lands.
    pub(crate) fn set(&mut self, ctx: &egui::Context, key: CoverageKey, image: egui::ColorImage) {
        self.job = None;
        match self.texture.as_mut() {
            Some((texture, shown)) => {
                texture.set(image, egui::TextureOptions::LINEAR);
                *shown = key;
            }
            None => {
                let texture = ctx.load_texture(
                    "selected-mask-coverage",
                    image,
                    egui::TextureOptions::LINEAR,
                );
                self.texture = Some((texture, key));
            }
        }
    }

    /// Draws `key` with `draw` on a worker, unless a draw is in flight. A
    /// worker that cannot be started leaves the texture as it is, and the
    /// next frame asks again.
    pub(crate) fn request(
        &mut self,
        ctx: &egui::Context,
        key: CoverageKey,
        draw: impl FnOnce() -> egui::ColorImage + Send + 'static,
    ) {
        if self.job.is_some() {
            // The first frame after it lands asks again for the newest key.
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let repaint = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("calibraw-mask-coverage".to_owned())
            .spawn(move || {
                // A cleared overlay has dropped the receiver; nothing waits.
                let _ = sender.send(draw());
                repaint.request_repaint();
            });
        if spawned.is_ok() {
            self.job = Some(CoverageJob { key, receiver });
        }
    }
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
    pub(crate) coverage: CoverageOverlay,
    pub(crate) overlay_blink: Option<(std::time::Instant, MaskOverlayBlink)>,
    pub(crate) thumbnail_revision: u64,
    pub(crate) thumbnail_group_textures: Vec<egui::TextureHandle>,
    pub(crate) thumbnail_component_mask: Option<usize>,
    pub(crate) thumbnail_component_textures: Vec<egui::TextureHandle>,
    pub(crate) source_cache: Option<MaskRgbImage>,
    pub(crate) subject_cache: Option<MaskImage>,
    pub(crate) sky_cache: Option<MaskImage>,
    pub(crate) depth_cache: Option<SceneDepth>,
    pub(crate) dirty_layers: [bool; MAX_LOCAL_MASKS],
    pub(crate) detail_dirty_layers: [bool; MAX_LOCAL_MASKS],
    pub(crate) navigation_dirty_layers: [bool; MAX_LOCAL_MASKS],
}
