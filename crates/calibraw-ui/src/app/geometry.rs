//! Geometry edits that refit the crop: the straighten rotation, from the straighten line or an
//! automatic estimate, and the perspective correction, and running the automatic estimates as
//! foreground operations.

use super::*;
use crate::pipeline::{
    estimate_perspective, estimate_straighten_rotation, LineAnalysisImage, PerspectiveCorrection,
};
use std::sync::atomic::Ordering;

/// What an automatic geometry job estimates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AutoGeometry {
    /// The straighten rotation that levels the photo.
    Straighten,
    /// The perspective correction and straighten rotation that make its lines upright.
    Perspective,
}

impl AutoGeometry {
    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Straighten => "Straightening",
            Self::Perspective => "Correcting perspective",
        }
    }

    const fn error_kind(self) -> ErrorKind {
        match self {
            Self::Straighten => ErrorKind::Straighten,
            Self::Perspective => ErrorKind::Perspective,
        }
    }

    /// Whether the estimate depends on `geometry` only through what `started` also has: the
    /// flips and skews, and for straightening the perspective correction it keeps.
    fn still_applies(self, started: GeometryTransform, current: GeometryTransform) -> bool {
        current.flip_horizontal == started.flip_horizontal
            && current.flip_vertical == started.flip_vertical
            && current.horizontal_skew_degrees == started.horizontal_skew_degrees
            && current.vertical_skew_degrees == started.vertical_skew_degrees
            && (self == Self::Perspective || current.perspective == started.perspective)
    }
}

/// The geometry an automatic job suggests, replacing the current rotation and perspective
/// correction; straightening keeps the correction it started with.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AutoGeometryEstimate {
    rotation_degrees: f32,
    perspective: PerspectiveCorrection,
}

/// The worker's outcome: an estimate, `None` when the photo has no clear straight edges, or a
/// message describing why the image could not be analysed.
pub(crate) type AutoGeometryResult = Result<Option<AutoGeometryEstimate>, String>;

impl CalibRawApp {
    /// Sets the fine rotation and refits the crop. Returns whether the rotation changed.
    pub(crate) fn set_straighten_rotation(
        &mut self,
        degrees: f32,
        source_width: u32,
        source_height: u32,
    ) -> bool {
        let previous = self.develop.geometry.rotation_degrees;
        self.develop.geometry.rotation_degrees = degrees.clamp(-45.0, 45.0);
        if (self.develop.geometry.rotation_degrees - previous).abs() <= 1e-4 {
            return false;
        }
        self.refit_crop(source_width, source_height);
        self.note_geometry_changed();
        true
    }

    /// Restores the last crop the user set, in the chosen aspect ratio, and fits it inside the
    /// transformed source, so repeated rotation, skew or perspective changes do not keep
    /// shrinking it. The ratio is applied again because the crop is stored relative to the
    /// corrected frame, whose shape the perspective correction changes.
    pub(crate) fn refit_crop(&mut self, source_width: u32, source_height: u32) {
        let reference = *self
            .develop_ui
            .crop_constraint_reference
            .get_or_insert(self.develop.geometry.crop);
        let geometry = &mut self.develop.geometry;
        geometry.crop = reference;
        geometry.conform_crop_to_aspect(source_width, source_height);
        geometry.fit_crop_inside_transformed_source(source_width, source_height);
    }

    /// The 35 mm-equivalent focal length a new perspective correction of the open photo is
    /// made with.
    pub(crate) fn perspective_focal_length(&self) -> f32 {
        PerspectiveCorrection::focal_length_35mm_for(
            self.develop
                .original_raw
                .as_ref()
                .map_or(0.0, |raw| raw.focal_length),
            self.develop.lens_correction.catalog.camera_crop_factor,
        )
    }

    /// Whether an automatic geometry job can start: a photo is open, no other foreground job
    /// runs and no lens correction is about to replace the source pixels.
    pub(crate) fn auto_geometry_available(&self) -> bool {
        self.develop.loaded_raw.is_some()
            && !self.foreground_operation_active()
            && !self.lens_correction_busy()
    }

    pub(crate) fn start_auto_geometry(&mut self, kind: AutoGeometry) {
        if !self.auto_geometry_available() {
            return;
        }
        let Some(raw) = self.develop.loaded_raw.as_ref().map(Arc::clone) else {
            return;
        };
        let geometry = self.develop.geometry;
        let focal_length = self.perspective_focal_length();
        let cancellation = Arc::new(AtomicBool::new(false));
        let worker_cancellation = Arc::clone(&cancellation);
        let repaint = self.egui_ctx.clone();
        let (sender, receiver) = mpsc::channel();
        let spawn_result = std::thread::Builder::new()
            .name("calibraw-auto-geometry".to_owned())
            .spawn(move || {
                let result = LineAnalysisImage::from_raw(&raw)
                    .map_err(|error| format!("Could not analyse the photo: {error:#}"))
                    .map(|image| {
                        // A cancelled job's result is discarded when polled, so any value works.
                        if worker_cancellation.load(Ordering::Acquire) {
                            None
                        } else {
                            estimate(kind, &image, geometry, focal_length)
                        }
                    });
                let _ = sender.send(result);
                repaint.request_repaint();
            });
        match spawn_result {
            Ok(_) => {
                self.begin_foreground_operation(ForegroundOperation {
                    kind: ForegroundOperationKind::AutoGeometry(kind),
                    document_id: self.persistence.document_generation,
                    cancellation,
                    progress: ForegroundProgress::indeterminate("Looking for straight lines…"),
                    cancelling: false,
                    receiver: ForegroundOperationReceiver::AutoGeometry(receiver),
                    context: ForegroundOperationContext::AutoGeometry { geometry },
                });
            }
            Err(error) => {
                self.report_error(
                    kind.error_kind(),
                    format!("Could not start {}: {error}", kind.title().to_lowercase()),
                );
            }
        }
    }

    pub(in crate::app) fn poll_auto_geometry_worker(&mut self, kind: AutoGeometry) {
        if !self.foreground_operation_is(ForegroundOperationKind::AutoGeometry(kind)) {
            return;
        }
        let Some(operation) = self.foreground_operation.take() else {
            return;
        };
        let ForegroundOperationReceiver::AutoGeometry(receiver) = &operation.receiver else {
            self.foreground_operation = Some(operation);
            return;
        };
        let (events, disconnected) = drain_worker_events(Some(receiver), |_| true);
        let result = match events.into_iter().next() {
            Some(result) => result,
            None if disconnected => Err(format!(
                "The {} worker stopped unexpectedly.",
                kind.title().to_lowercase()
            )),
            None => {
                self.foreground_operation = Some(operation);
                return;
            }
        };
        if !operation.accepts_result(self.persistence.document_generation) {
            return;
        }
        let ForegroundOperationContext::AutoGeometry { geometry } = operation.context else {
            return;
        };
        if !kind.still_applies(geometry, self.develop.geometry) {
            self.ui.notice =
                Some("The transform changed while the photo was analysed; try again.".to_owned());
            return;
        }
        match result {
            Ok(Some(estimate)) => self.apply_auto_geometry(kind, estimate),
            Ok(None) => {
                self.ui.notice = Some(
                    match kind {
                        AutoGeometry::Straighten => {
                            "No clear horizon or vertical edges to straighten by."
                        }
                        AutoGeometry::Perspective => "No clear straight edges to correct by.",
                    }
                    .to_owned(),
                );
            }
            Err(error) => self.report_error(kind.error_kind(), error),
        }
        self.egui_ctx.request_repaint();
    }

    fn apply_auto_geometry(&mut self, kind: AutoGeometry, estimate: AutoGeometryEstimate) {
        let Some((width, height)) = self
            .develop
            .loaded_raw
            .as_ref()
            .map(|raw| (raw.width, raw.height))
        else {
            return;
        };
        let previous = self.develop.geometry;
        let geometry = &mut self.develop.geometry;
        geometry.rotation_degrees = estimate.rotation_degrees.clamp(-45.0, 45.0);
        geometry.perspective = estimate.perspective.sanitized();
        if (geometry.rotation_degrees - previous.rotation_degrees).abs() <= 1e-4
            && geometry.perspective == previous.perspective.sanitized()
        {
            *geometry = previous;
            self.ui.notice = Some(
                match kind {
                    AutoGeometry::Straighten => "The photo is already level.",
                    AutoGeometry::Perspective => "The photo is already upright.",
                }
                .to_owned(),
            );
            return;
        }
        self.refit_crop(width, height);
        self.note_geometry_changed();
    }
}

fn estimate(
    kind: AutoGeometry,
    image: &LineAnalysisImage,
    geometry: GeometryTransform,
    focal_length_35mm: f32,
) -> Option<AutoGeometryEstimate> {
    match kind {
        AutoGeometry::Straighten => {
            estimate_straighten_rotation(image, geometry).map(|estimate| AutoGeometryEstimate {
                rotation_degrees: estimate.rotation_degrees,
                perspective: geometry.perspective,
            })
        }
        AutoGeometry::Perspective => {
            estimate_perspective(image, geometry, focal_length_35mm).map(|estimate| {
                AutoGeometryEstimate {
                    rotation_degrees: estimate.rotation_degrees,
                    perspective: estimate.perspective,
                }
            })
        }
    }
}
