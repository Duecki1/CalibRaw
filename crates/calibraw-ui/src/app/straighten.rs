//! Straightening: applying a levelling rotation, from the straighten line or the automatic
//! estimate, and running that estimate as a foreground operation.

use super::*;
use crate::pipeline::{estimate_straighten_rotation, LineAnalysisImage, StraightenEstimate};
use std::sync::atomic::Ordering;

/// The worker's outcome: an estimate, `None` when the photo has no clear level edges, or a
/// message describing why the image could not be analysed.
pub(crate) type AutoStraightenResult = Result<Option<StraightenEstimate>, String>;

impl CalibRawApp {
    /// Sets the fine rotation and refits the crop from the last crop the user set, so repeated
    /// straightening does not keep shrinking it. Returns whether the rotation changed.
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
        let reference = *self
            .develop_ui
            .crop_constraint_reference
            .get_or_insert(self.develop.geometry.crop);
        self.develop.geometry.crop = reference;
        self.develop
            .geometry
            .fit_crop_inside_transformed_source(source_width, source_height);
        self.note_geometry_changed();
        true
    }

    /// Whether automatic straightening can start: a photo is open, no other foreground job runs
    /// and no lens correction is about to replace the source pixels.
    pub(crate) fn auto_straighten_available(&self) -> bool {
        self.develop.loaded_raw.is_some()
            && !self.foreground_operation_active()
            && !self.lens_correction_busy()
    }

    pub(crate) fn start_auto_straighten(&mut self) {
        if !self.auto_straighten_available() {
            return;
        }
        let Some(raw) = self.develop.loaded_raw.as_ref().map(Arc::clone) else {
            return;
        };
        let geometry = self.develop.geometry;
        let cancellation = Arc::new(AtomicBool::new(false));
        let worker_cancellation = Arc::clone(&cancellation);
        let repaint = self.egui_ctx.clone();
        let (sender, receiver) = mpsc::channel();
        let spawn_result = std::thread::Builder::new()
            .name("calibraw-auto-straighten".to_owned())
            .spawn(move || {
                let result = LineAnalysisImage::from_raw(&raw)
                    .map_err(|error| format!("Could not analyse the photo: {error:#}"))
                    .map(|image| {
                        // A cancelled job's result is discarded when polled, so any value works.
                        if worker_cancellation.load(Ordering::Acquire) {
                            None
                        } else {
                            estimate_straighten_rotation(&image, geometry)
                        }
                    });
                let _ = sender.send(result);
                repaint.request_repaint();
            });
        match spawn_result {
            Ok(_) => {
                self.begin_foreground_operation(ForegroundOperation {
                    kind: ForegroundOperationKind::AutoStraighten,
                    document_id: self.persistence.document_generation,
                    cancellation,
                    progress: ForegroundProgress::indeterminate("Looking for level lines…"),
                    cancelling: false,
                    receiver: ForegroundOperationReceiver::AutoStraighten(receiver),
                    context: ForegroundOperationContext::AutoStraighten { geometry },
                });
            }
            Err(error) => {
                self.report_error(
                    ErrorKind::Straighten,
                    format!("Could not start straightening: {error}"),
                );
            }
        }
    }

    pub(in crate::app) fn poll_auto_straighten_worker(&mut self) {
        if !self.foreground_operation_is(ForegroundOperationKind::AutoStraighten) {
            return;
        }
        let Some(operation) = self.foreground_operation.take() else {
            return;
        };
        let ForegroundOperationReceiver::AutoStraighten(receiver) = &operation.receiver else {
            self.foreground_operation = Some(operation);
            return;
        };
        let (events, disconnected) = drain_worker_events(Some(receiver), |_| true);
        let result = match events.into_iter().next() {
            Some(result) => result,
            None if disconnected => {
                Err("The straightening worker stopped unexpectedly.".to_owned())
            }
            None => {
                self.foreground_operation = Some(operation);
                return;
            }
        };
        if !operation.accepts_result(self.persistence.document_generation) {
            return;
        }
        let ForegroundOperationContext::AutoStraighten { geometry } = operation.context else {
            return;
        };
        let current = self.develop.geometry;
        if current.flip_horizontal != geometry.flip_horizontal
            || current.flip_vertical != geometry.flip_vertical
            || current.horizontal_transform != geometry.horizontal_transform
            || current.vertical_transform != geometry.vertical_transform
        {
            self.ui.notice =
                Some("The transform changed while straightening; try again.".to_owned());
            return;
        }
        match result {
            Ok(Some(estimate)) => {
                if let Some((width, height)) = self
                    .develop
                    .loaded_raw
                    .as_ref()
                    .map(|raw| (raw.width, raw.height))
                {
                    if !self.set_straighten_rotation(estimate.rotation_degrees, width, height) {
                        self.ui.notice = Some("The photo is already level.".to_owned());
                    }
                }
            }
            Ok(None) => {
                self.ui.notice =
                    Some("No clear horizon or vertical edges to straighten by.".to_owned());
            }
            Err(error) => self.report_error(ErrorKind::Straighten, error),
        }
        self.egui_ctx.request_repaint();
    }
}
