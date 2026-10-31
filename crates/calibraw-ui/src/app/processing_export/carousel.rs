//! Exporting a carousel: one render, one file per slide.
//!
//! The render worker moves desktop files into place itself. Where exports go through the media
//! store, the finished slides are published in order and one at a time, as a gallery publish
//! reports back asynchronously and a second one would replace its permission request.

use super::*;
use crate::i18n::{tr, tr_fmt};
use crate::pipeline::Carousel;
use crate::platform::{ExportPublishResult, PublishedExport};

/// The media-store publishing of a finished carousel.
pub(crate) struct SlidePublish {
    pending: VecDeque<ExportDestination>,
    published: Vec<PublishedExport>,
    total: usize,
}

impl CalibRawApp {
    /// Exports `carousel` of the open photo: into a chosen folder, or into the platform's shared
    /// media store, opening the share sheet afterwards when `share` is set.
    pub(super) fn export_carousel(
        &mut self,
        frame: &eframe::Frame,
        carousel: Carousel,
        format: ExportFormat,
        share: bool,
    ) {
        if !self.can_export() {
            return;
        }
        let Some(stem) = self.templated_export_stem() else {
            return;
        };
        let slide_count = usize::from(carousel.slides());
        let slides = if self.caps().has_folders() {
            let Some(slides) = self.choose_carousel_destinations(&stem, format, slide_count) else {
                return;
            };
            slides
        } else {
            match self.prepare_android_carousel_destinations(&stem, format, slide_count) {
                Ok(slides) => slides,
                Err(error) => {
                    self.report_error(ErrorKind::Export, error);
                    return;
                }
            }
        };
        let output = ExportItemOutput::Carousel {
            carousel,
            slides: slides.clone(),
        };
        if self.start_export_output(output, frame, format).is_none() {
            for slide in &slides {
                self.cancel_android_export_destination(slide);
            }
            return;
        }
        self.export.share_mime_type = share.then_some(format.mime_type());
    }

    /// New files `{stem}-1.{extension}` onwards in a chosen folder. The stem is chosen so that
    /// none of them is taken, which keeps a carousel's numbering together.
    fn choose_carousel_destinations(
        &self,
        stem: &str,
        format: ExportFormat,
        slide_count: usize,
    ) -> Option<Vec<ExportDestination>> {
        let mut dialog = crate::platform::FileDialog::new();
        if let Some(parent) = self
            .develop
            .current_path
            .as_deref()
            .and_then(Path::parent)
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            dialog = dialog.set_directory(parent);
        }
        let folder = dialog.pick_folder()?;
        let extension = format.extension();
        let stem = crate::export_naming::free_carousel_stem(&folder, stem, extension, slide_count);
        Some(
            (1..=slide_count)
                .map(|slide| ExportDestination::NewFile {
                    directory: folder.clone(),
                    stem: format!("{stem}-{slide}"),
                    extension: extension.to_owned(),
                })
                .collect(),
        )
    }

    fn prepare_android_carousel_destinations(
        &self,
        stem: &str,
        format: ExportFormat,
        slide_count: usize,
    ) -> Result<Vec<ExportDestination>, String> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let extension = format.extension();
        let mut slides = Vec::with_capacity(slide_count);
        for slide in 1..=slide_count {
            let prepared = self.prepare_android_export_destination(
                format!("{stem}-{slide}.{extension}"),
                format!("{stem}-{slide}-{timestamp}.{extension}"),
                format,
            );
            match prepared {
                Ok(destination) => slides.push(destination),
                Err(error) => {
                    for destination in &slides {
                        self.cancel_android_export_destination(destination);
                    }
                    return Err(error);
                }
            }
        }
        Ok(slides)
    }

    /// The worker finished every slide; `first_slide` is where the first one went.
    pub(super) fn finish_carousel_render(
        &mut self,
        first_slide: &Path,
        slides: Vec<ExportDestination>,
    ) {
        if self.caps().has_media_library() {
            self.export.slide_publish = Some(SlidePublish {
                total: slides.len(),
                pending: slides.into(),
                published: Vec::new(),
            });
            self.publish_next_slides();
        } else {
            let folder = first_slide.parent().unwrap_or(first_slide);
            self.ui.notice = Some(tr_fmt(
                "Exported {count} slides to {location}",
                &[("count", &slides.len()), ("location", &folder.display())],
            ));
            self.export.task = None;
        }
    }

    /// Publishes slides until one has to wait for the platform or none is left.
    fn publish_next_slides(&mut self) {
        while let Some(slide) = self
            .export
            .slide_publish
            .as_mut()
            .and_then(|publish| publish.pending.pop_front())
        {
            match slide {
                ExportDestination::AndroidDirect { path, .. } => {
                    match self.platform.finalize_direct_export(&path) {
                        Ok(published) => self.note_slide_published(published),
                        Err(error) => return self.fail_slide_publish(error),
                    }
                }
                ExportDestination::AndroidGallery {
                    path,
                    display_name,
                    format,
                } => {
                    match self
                        .platform
                        .publish_image(&path, &display_name, format.mime_type())
                    {
                        Ok(()) => {
                            self.export.publish_pending = true;
                            if let Some(task) = self.export.task.as_mut() {
                                task.progress = task.progress.max(EXPORT_MAX_INCOMPLETE_FRACTION);
                                task.phase = tr_fmt(
                                    "Publishing to {location}…",
                                    &[("location", &self.platform.export_location_label())],
                                );
                            }
                            return;
                        }
                        Err(error) => {
                            let _ = std::fs::remove_file(&path);
                            return self.fail_slide_publish(error);
                        }
                    }
                }
                ExportDestination::File(_) | ExportDestination::NewFile { .. } => {
                    return self
                        .fail_slide_publish(tr("export destination state was lost").to_owned());
                }
            }
        }
        self.finish_slide_publish();
    }

    /// Takes the platform's report on a gallery publish while a carousel is being published;
    /// otherwise hands `result` back.
    pub(in crate::app) fn take_slide_publish_result(
        &mut self,
        result: ExportPublishResult,
    ) -> Option<ExportPublishResult> {
        if self.export.slide_publish.is_none() {
            return Some(result);
        }
        match result {
            ExportPublishResult::Published(published) => {
                self.note_slide_published(published);
                self.publish_next_slides();
            }
            ExportPublishResult::Failed(error) => self.fail_slide_publish(error),
        }
        None
    }

    fn note_slide_published(&mut self, published: PublishedExport) {
        if let Some(publish) = self.export.slide_publish.as_mut() {
            publish.published.push(published);
        }
    }

    fn finish_slide_publish(&mut self) {
        let Some(publish) = self.export.slide_publish.take() else {
            return;
        };
        let location = self.platform.export_location_label();
        self.ui.notice = Some(tr_fmt(
            "Exported {count} slides to {location}",
            &[("count", &publish.total), ("location", &location)],
        ));
        self.share_finished_slides(&publish.published, location);
        self.export.task = None;
    }

    /// Opens the share sheet for every slide when the export asked for it.
    fn share_finished_slides(&mut self, published: &[PublishedExport], location: &str) {
        let Some(mime_type) = self.export.share_mime_type.take() else {
            return;
        };
        if published.iter().any(|slide| slide.uri.is_empty()) {
            self.report_error(
                ErrorKind::Share,
                tr_fmt(
                    "Exported to {location}, but Android did not provide a link to share it.",
                    &[("location", &location)],
                ),
            );
            return;
        }
        let uris = published
            .iter()
            .map(|slide| slide.uri.clone())
            .collect::<Vec<_>>();
        if let Err(error) = self.platform.share_exports(&uris, mime_type) {
            self.report_error(
                ErrorKind::Share,
                tr_fmt(
                    "Exported to {location}, but sharing failed: {error}",
                    &[("location", &location), ("error", &error)],
                ),
            );
        }
    }

    /// Drops the slides not published yet. Those already published stay in the gallery, and the
    /// message says how many.
    fn fail_slide_publish(&mut self, error: String) {
        let Some(publish) = self.export.slide_publish.take() else {
            return;
        };
        for slide in &publish.pending {
            match slide {
                ExportDestination::AndroidGallery { path, .. } => {
                    let _ = std::fs::remove_file(path);
                }
                other => self.cancel_android_export_destination(other),
            }
        }
        self.export.share_mime_type = None;
        self.export.task = None;
        self.report_error(
            ErrorKind::Export,
            tr_fmt(
                "Export failed after {done} of {total} slides: {error}",
                &[
                    ("done", &publish.published.len()),
                    ("total", &publish.total),
                    ("error", &error),
                ],
            ),
        );
    }
}
