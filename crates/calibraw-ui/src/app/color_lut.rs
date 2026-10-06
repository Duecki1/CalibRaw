//! The colour look (a 3D LUT) of the open photo: choosing, removing and
//! re-rendering it.
use super::*;

impl CalibRawApp {
    /// Re-renders the preview after the look changed. The look is not part of
    /// `ExposureParams`, so `mark_pipeline_dirty` would find nothing to do.
    pub(crate) fn mark_color_lut_dirty(&mut self) {
        self.note_edit_changed();
        if self.preview.gpu_pipeline.is_some() {
            self.queue_preview_processing(ProcessingStage::Output);
        }
    }

    /// Installs `look`, or removes the current one with `None`.
    pub(crate) fn set_color_lut(&mut self, look: Option<ColorLutEdit>) {
        self.develop.color_lut = look;
        self.mark_color_lut_dirty();
    }

    /// Whether a look can be chosen: a photo is open and loaded.
    pub(crate) fn can_edit_color_lut(&self) -> bool {
        self.develop.loaded_raw.is_some() && self.develop.load_receiver.is_none()
    }

    #[cfg(not(target_os = "android"))]
    pub(crate) fn choose_color_lut_to_import(&mut self) {
        if self.ui.desktop_picker_receiver.is_some() || !self.can_edit_color_lut() {
            return;
        }
        let dialog =
            rfd::AsyncFileDialog::new().add_filter("Colour lookup tables (.cube)", &["cube"]);
        self.ui.desktop_picker_receiver = Some(spawn_ui_worker(&self.egui_ctx, move || {
            // Parsing a 65-point table takes a moment, so it stays off the UI thread.
            let look = pollster::block_on(dialog.pick_file()).map(|handle| {
                let path = handle.path();
                ColorLutEdit::from_cube_file(path).map_err(|error| {
                    format!(
                        "Could not read {}: {error}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    )
                })
            });
            DesktopPickerEvent::ColorLut(look)
        }));
    }
}
