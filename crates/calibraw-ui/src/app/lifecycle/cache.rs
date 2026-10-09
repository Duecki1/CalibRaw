use super::*;

impl CalibRawApp {
    /// Drops every decoded RAW, for settings that change what decodes produce,
    /// and stops a prefetch still decoding with the old settings.
    pub(in crate::app) fn clear_decoded_raws(&mut self) {
        #[cfg(not(target_os = "android"))]
        self.cancel_neighbour_prefetch();
        self.develop.decoded_raws.clear();
    }

    pub(in crate::app) fn new_image_exposure(&self) -> ExposureParams {
        let previous = self.develop.exposure;
        let mut exposure = ExposureParams::scene_referred_default();

        exposure.highlight_clip = previous.highlight_clip;
        exposure.highlight_reconstruction = previous.highlight_reconstruction;
        exposure.demosaic_mode = previous.demosaic_mode;
        exposure.dual_threshold = previous.dual_threshold;
        exposure.frequency_chroma = previous.frequency_chroma;
        exposure
    }

    #[cfg(not(target_os = "android"))]
    pub(in crate::app) fn prepare_develop_loading_thumbnail(&mut self, path: &std::path::Path) {
        const LOADING_THUMBNAIL_EDGE: u32 = 512;

        self.develop_ui.loading_thumbnail.clear();
        self.ui.adaptive_preview_backdrop = crate::ui::theme::CANVAS_BACKDROP;
        self.develop_ui.loading_thumbnail.path = Some(path.to_owned());

        if let Some((texture, size, adaptive_backdrop)) = self
            .library
            .desktop_loading_thumbnail_for_path(path, &self.egui_ctx)
        {
            self.develop_ui.loading_thumbnail.texture = Some(texture);
            self.develop_ui.loading_thumbnail.texture_size = Some(size);
            if let Some(color) = adaptive_backdrop {
                self.ui.adaptive_preview_backdrop = color;
            }
        }

        if self.develop_ui.loading_thumbnail.texture.is_some() {
            return;
        }

        let worker_path = path.to_owned();
        let repaint = self.egui_ctx.clone();
        let (sender, receiver) = mpsc::channel();
        match std::thread::Builder::new()
            .name("calibraw-loading-thumbnail".to_owned())
            .spawn(move || {
                let result = crate::ui::library::load_desktop_cached_thumbnail(
                    &worker_path,
                    LOADING_THUMBNAIL_EDGE,
                );
                let _ = sender.send((worker_path, result));
                repaint.request_repaint();
            }) {
            Ok(_) => self.develop_ui.loading_thumbnail.receiver = Some(receiver),
            Err(error) => log::warn!("could not start loading-thumbnail worker: {error}"),
        }
    }

    #[cfg(not(target_os = "android"))]
    pub(crate) fn refresh_develop_loading_thumbnail(&mut self, context: &egui::Context) {
        let Some(path) = self.develop_ui.loading_thumbnail.path.clone() else {
            return;
        };

        self.library.poll(context);
        if self.develop_ui.loading_thumbnail.texture.is_none() {
            if let Some((texture, size, adaptive_backdrop)) = self
                .library
                .desktop_loading_thumbnail_for_path(&path, context)
            {
                self.develop_ui.loading_thumbnail.texture = Some(texture);
                self.develop_ui.loading_thumbnail.texture_size = Some(size);
                if let Some(color) = adaptive_backdrop {
                    self.ui.adaptive_preview_backdrop = color;
                }
            }
        }

        let event = self
            .develop_ui
            .loading_thumbnail
            .receiver
            .as_ref()
            .and_then(|receiver| match receiver.try_recv() {
                Ok(event) => Some(Ok(event)),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(())),
            });
        let Some(event) = event else {
            return;
        };
        self.develop_ui.loading_thumbnail.receiver = None;

        let Ok((loaded_path, result)) = event else {
            return;
        };
        if loaded_path != path || self.develop_ui.loading_thumbnail.texture.is_some() {
            return;
        }
        match result {
            Ok(Some(thumbnail)) => {
                self.ui.adaptive_preview_backdrop =
                    crate::ui::theme::adaptive_backdrop_from_rgba(&thumbnail.rgba);
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [thumbnail.width as usize, thumbnail.height as usize],
                    &thumbnail.rgba,
                );
                self.develop_ui.loading_thumbnail.texture = Some(context.load_texture(
                    format!("develop-loading-thumbnail-{}", loaded_path.display()),
                    image,
                    egui::TextureOptions::LINEAR,
                ));
                self.develop_ui.loading_thumbnail.texture_size =
                    Some([thumbnail.width, thumbnail.height]);
            }
            Ok(None) => {}
            Err(error) => log::warn!(
                "could not load cached thumbnail for {}: {error}",
                loaded_path.display()
            ),
        }
    }

    #[cfg(target_os = "android")]
    pub(in crate::app) fn prepare_android_develop_loading_thumbnail(&mut self, uri: &str) {
        self.develop_ui.loading_thumbnail.clear();
        self.ui.adaptive_preview_backdrop = crate::ui::theme::CANVAS_BACKDROP;
        self.develop_ui.loading_thumbnail.source_uri = Some(uri.to_owned());
        if let Some((texture, size, adaptive_backdrop)) = self
            .library
            .android_loading_thumbnail_for_uri(uri, &self.egui_ctx)
        {
            self.develop_ui.loading_thumbnail.texture = Some(texture);
            self.develop_ui.loading_thumbnail.texture_size = Some(size);
            if let Some(color) = adaptive_backdrop {
                self.ui.adaptive_preview_backdrop = color;
            }
        }
    }

    #[cfg(target_os = "android")]
    pub(crate) fn refresh_develop_loading_thumbnail(&mut self, context: &egui::Context) {
        let Some(uri) = self.develop_ui.loading_thumbnail.source_uri.clone() else {
            return;
        };
        self.library.poll(context);
        if self.develop_ui.loading_thumbnail.texture.is_none() {
            if let Some((texture, size, adaptive_backdrop)) = self
                .library
                .android_loading_thumbnail_for_uri(&uri, context)
            {
                self.develop_ui.loading_thumbnail.texture = Some(texture);
                self.develop_ui.loading_thumbnail.texture_size = Some(size);
                if let Some(color) = adaptive_backdrop {
                    self.ui.adaptive_preview_backdrop = color;
                }
            }
        }
    }
}
