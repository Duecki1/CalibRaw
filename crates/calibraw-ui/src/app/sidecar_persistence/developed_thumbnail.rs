//! Refreshing the library's developed thumbnail after edits are saved.

use super::*;

impl CalibRawApp {
    pub(in crate::app) fn install_developed_thumbnail_result(
        &mut self,
        target: &crate::sidecar::SidecarTarget,
        thumbnail: crate::pipeline::RawThumbnail,
        revision: u64,
    ) {
        match target {
            #[cfg(not(target_os = "android"))]
            crate::sidecar::SidecarTarget::Desktop { raw_path } => {
                self.library.install_developed_thumbnail(
                    raw_path,
                    thumbnail,
                    &self.egui_ctx,
                    revision,
                );
            }
            #[cfg(target_os = "android")]
            crate::sidecar::SidecarTarget::Desktop { .. } => {}
            #[cfg(target_os = "android")]
            crate::sidecar::SidecarTarget::Android { raw_uri, .. } => {
                self.library.install_android_developed_thumbnail(
                    raw_uri,
                    thumbnail,
                    &self.egui_ctx,
                    revision,
                );
            }
        }
    }

    pub(in crate::app) fn load_developed_thumbnail_for_target(
        &self,
        target: &crate::sidecar::SidecarTarget,
    ) -> Result<Option<crate::pipeline::RawThumbnail>, String> {
        match target {
            #[cfg(not(target_os = "android"))]
            crate::sidecar::SidecarTarget::Desktop { raw_path } => {
                crate::sidecar::load_developed_thumbnail_cache(raw_path, 512)
            }
            #[cfg(target_os = "android")]
            crate::sidecar::SidecarTarget::Desktop { .. } => Ok(None),
            #[cfg(target_os = "android")]
            crate::sidecar::SidecarTarget::Android {
                raw_uri,
                display_name,
            } => calibraw_ffi::load_developed_thumbnail_cache(
                &self.android.android_app,
                raw_uri,
                display_name,
                512,
            ),
        }
    }

    pub(in crate::app) fn queue_developed_thumbnail_refresh(
        &mut self,
        generation: u64,
        revision: u64,
    ) {
        if generation != self.persistence.document_generation {
            return;
        }
        let Some(target) = self.persistence.sidecar_target.clone() else {
            return;
        };
        let job = DevelopedThumbnailJob {
            target,
            generation,
            revision,
        };

        match self.load_developed_thumbnail_for_target(&job.target) {
            Ok(Some(thumbnail)) => {
                self.install_developed_thumbnail_result(&job.target, thumbnail, revision);
                if self.persistence.developed_thumbnail_pending.as_ref() == Some(&job) {
                    self.persistence.developed_thumbnail_pending = None;
                }
                return;
            }
            Ok(None) => {}
            Err(error) => {
                log::warn!("could not validate developed thumbnail cache: {error}");
            }
        }

        if self.persistence.developed_thumbnail_in_flight.as_ref() == Some(&job)
            || self.persistence.developed_thumbnail_pending.as_ref() == Some(&job)
        {
            return;
        }
        self.persistence.developed_thumbnail_pending = Some(job);
        self.egui_ctx.request_repaint();
    }

    pub(in crate::app) fn poll_developed_thumbnail(&mut self, frame: &eframe::Frame) {
        let received = self
            .persistence
            .developed_thumbnail_receiver
            .as_ref()
            .map(mpsc::Receiver::try_recv);
        match received {
            Some(Ok(event)) => {
                self.persistence.developed_thumbnail_receiver = None;
                self.persistence.developed_thumbnail_in_flight = None;
                match event.result {
                    Ok(thumbnail) => self.install_developed_thumbnail_result(
                        &event.job.target,
                        thumbnail,
                        event.job.revision,
                    ),
                    Err(error) => {
                        if error.contains("sidecar changed") {
                            log::debug!("discarded stale developed thumbnail: {error}");
                        } else {
                            log::warn!("could not refresh developed thumbnail: {error}");
                        }
                    }
                }
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.persistence.developed_thumbnail_receiver = None;
                self.persistence.developed_thumbnail_in_flight = None;
                log::warn!("developed-thumbnail worker stopped unexpectedly");
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }

        if self.persistence.developed_thumbnail_in_flight.is_some() {
            return;
        }
        let Some(job) = self.persistence.developed_thumbnail_pending.clone() else {
            return;
        };
        if job.generation != self.persistence.document_generation
            || self.persistence.sidecar_target.as_ref() != Some(&job.target)
        {
            self.persistence.developed_thumbnail_pending = None;
            return;
        }
        let current_revision = self.edit_commit_revision();
        if current_revision != job.revision
            || self.persistence.sidecar_saved_revision != Some(job.revision)
        {
            self.persistence.developed_thumbnail_pending = None;
            return;
        }
        if self.preview.quality_dirty
            || self.develop.lens_correction_dirty
            || self.lens_correction_busy()
        {
            self.egui_ctx.request_repaint();
            return;
        }

        if !edited_preview_is_current_for_thumbnail(
            self.preview.original_requested,
            self.preview.original_rendered_state,
            self.preview.revision,
        ) {
            self.egui_ctx.request_repaint();
            return;
        }

        let Some(render_state) = frame.wgpu_render_state() else {
            self.persistence.developed_thumbnail_pending = None;
            log::warn!("cannot cache developed thumbnail without the wgpu backend");
            return;
        };
        let snapshot = if self.preview.pending_stage.is_none() {
            self.preview
                .pipeline()
                .map(|pipeline| pipeline.output_snapshot(&render_state.device, &render_state.queue))
        } else if self.preview.navigation_pending_stage.is_none() {
            self.preview.navigation.as_ref().map(|preview| {
                preview
                    .pipeline
                    .gpu()
                    .output_snapshot(&render_state.device, &render_state.queue)
            })
        } else {
            None
        };
        let Some(snapshot) = snapshot else {
            self.egui_ctx.request_repaint();
            return;
        };
        let device = render_state.device.clone();
        let queue = render_state.queue.clone();
        let repaint = self.egui_ctx.clone();
        let geometry = self.develop.geometry;
        let worker_job = job.clone();
        let worker_target = job.target.clone();
        #[cfg(target_os = "android")]
        let android_app = self.android.android_app.clone();
        let (sender, receiver) = mpsc::channel();
        let spawn = std::thread::Builder::new()
            .name("calibraw-developed-thumbnail".to_owned())
            .spawn(move || {
                let result = (|| {
                    let thumbnail = snapshot
                        .read_thumbnail_blocking(&device, &queue, 512)
                        .map_err(|error| format!("GPU thumbnail readback failed: {error:#}"))?;
                    let thumbnail =
                        crate::pipeline::transform_thumbnail_geometry(&thumbnail, geometry);
                    match &worker_target {
                        #[cfg(not(target_os = "android"))]
                        crate::sidecar::SidecarTarget::Desktop { raw_path } => {
                            let fingerprint = crate::sidecar::desktop_sidecar_fingerprint(
                                raw_path,
                            )?
                            .ok_or_else(|| {
                                "edit sidecar disappeared before thumbnail capture".to_owned()
                            })?;
                            crate::sidecar::save_developed_thumbnail_cache(
                                raw_path,
                                &thumbnail,
                                fingerprint,
                            )?;
                        }
                        #[cfg(target_os = "android")]
                        crate::sidecar::SidecarTarget::Desktop { .. } => {}
                        #[cfg(target_os = "android")]
                        crate::sidecar::SidecarTarget::Android {
                            raw_uri,
                            display_name,
                        } => calibraw_ffi::save_developed_thumbnail_cache(
                            &android_app,
                            raw_uri,
                            display_name,
                            &thumbnail,
                        )?,
                    }
                    Ok(thumbnail)
                })();
                let _ = sender.send(DevelopedThumbnailEvent {
                    job: worker_job,
                    result,
                });
                repaint.request_repaint();
            });
        match spawn {
            Ok(_) => {
                self.persistence.developed_thumbnail_pending = None;
                self.persistence.developed_thumbnail_in_flight = Some(job);
                self.persistence.developed_thumbnail_receiver = Some(receiver);
            }
            Err(error) => {
                self.persistence.developed_thumbnail_pending = None;
                log::warn!("could not start developed-thumbnail worker: {error}");
            }
        }
    }
}
