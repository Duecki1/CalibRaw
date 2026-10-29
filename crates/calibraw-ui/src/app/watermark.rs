//! The export watermark: an image placed over every export while it is on.
//!
//! The Export panel's card returns [`WatermarkAction`]s and the preview
//! draws the watermark while that panel is open; the handlers here change
//! the state. The chosen image is kept as `watermark.png` beside the
//! settings file and the switch and placement are saved with the settings,
//! so the watermark is reused for other photos and batch exports. It is not
//! part of any photo's edit.

use super::*;
use crate::performance_settings::SavedWatermark;
use crate::pipeline::{ExportSettings, ExportWatermark, WatermarkImage, WatermarkPlacement};

/// Largest image file accepted as a watermark.
const MAX_WATERMARK_FILE_BYTES: u64 = 64 * 1024 * 1024;
const WATERMARK_FILE_NAME: &str = "watermark.png";

type WatermarkLoad = Result<Arc<WatermarkImage>, String>;

pub(crate) struct WatermarkState {
    enabled: bool,
    placement: WatermarkPlacement,
    image: Option<Arc<WatermarkImage>>,
    /// The image, uploaded for the preview.
    texture: Option<egui::TextureHandle>,
    loading: Option<PendingWatermarkLoad>,
    /// The placement changed since the settings were last saved.
    placement_unsaved: bool,
}

struct PendingWatermarkLoad {
    receiver: mpsc::Receiver<WatermarkLoad>,
    source: WatermarkSource,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WatermarkSource {
    /// The saved image, read at start-up. Failing turns the watermark off.
    Saved,
    /// A newly chosen image. Succeeding turns the watermark on.
    Chosen,
}

/// What the Export panel's card and the preview show.
#[derive(Clone, Copy)]
pub(crate) struct WatermarkView {
    pub(crate) enabled: bool,
    pub(crate) placement: WatermarkPlacement,
    pub(crate) loading: bool,
    /// The uploaded image and its size in pixels.
    pub(crate) image: Option<(egui::TextureId, [u32; 2])>,
}

impl WatermarkView {
    /// The image while the watermark is on.
    pub(crate) fn shown_image(&self) -> Option<(egui::TextureId, [u32; 2])> {
        self.image.filter(|_| self.enabled)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum WatermarkAction {
    SetEnabled(bool),
    ChooseImage,
    Place(WatermarkPlacement),
}

impl WatermarkState {
    pub(in crate::app) fn new(saved: SavedWatermark) -> Self {
        Self {
            enabled: saved.enabled,
            placement: WatermarkPlacement::new(saved.anchor, saved.margin, saved.size),
            image: None,
            texture: None,
            loading: None,
            placement_unsaved: false,
        }
    }

    pub(in crate::app) fn saved(&self) -> SavedWatermark {
        SavedWatermark {
            enabled: self.enabled,
            anchor: self.placement.anchor(),
            margin: self.placement.margin(),
            size: self.placement.size(),
        }
    }

    pub(crate) fn view(&self) -> WatermarkView {
        WatermarkView {
            enabled: self.enabled,
            placement: self.placement,
            loading: self.loading.is_some(),
            image: self
                .image
                .as_ref()
                .zip(self.texture.as_ref())
                .map(|(image, texture)| (texture.id(), [image.width(), image.height()])),
        }
    }

    /// `settings` with the watermark an export draws, if it is on.
    pub(in crate::app) fn export_settings(&self, settings: &ExportSettings) -> ExportSettings {
        let watermark = self
            .image
            .as_ref()
            .filter(|_| self.enabled)
            .map(|image| ExportWatermark {
                image: Arc::clone(image),
                placement: self.placement,
            });
        ExportSettings {
            watermark,
            ..settings.clone()
        }
    }
}

impl CalibRawApp {
    /// Reads the saved watermark image on a worker. A missing image turns
    /// the watermark off.
    pub(in crate::app) fn load_saved_watermark(&mut self) {
        let Some(path) = self.watermark_path().filter(|path| path.is_file()) else {
            self.export.watermark.enabled = false;
            return;
        };
        self.start_watermark_load(WatermarkSource::Saved, move || read_watermark(&path));
    }

    pub(crate) fn apply_watermark_action(&mut self, action: WatermarkAction) {
        let watermark = &mut self.export.watermark;
        match action {
            WatermarkAction::SetEnabled(true) if watermark.image.is_none() => {
                self.choose_watermark_image();
            }
            WatermarkAction::SetEnabled(enabled) => {
                if watermark.enabled != enabled {
                    watermark.enabled = enabled;
                    self.persist_performance_settings();
                }
            }
            WatermarkAction::ChooseImage => self.choose_watermark_image(),
            WatermarkAction::Place(placement) => {
                if watermark.placement != placement {
                    watermark.placement = placement;
                    watermark.placement_unsaved = true;
                    // `poll_watermark` saves it on a frame after a slider is
                    // released.
                    self.egui_ctx.request_repaint();
                }
            }
        }
    }

    fn choose_watermark_image(&mut self) {
        if self.export.watermark.loading.is_some() {
            return;
        }
        if self.caps().has_folders() {
            if self.ui.desktop_picker_receiver.is_some() {
                return;
            }
            let dialog = crate::platform::FileDialog::new()
                .add_filter("Images", &["png", "jpg", "jpeg", "tif", "tiff"]);
            self.ui.desktop_picker_receiver = Some(spawn_ui_worker(&self.egui_ctx, move || {
                DesktopPickerEvent::WatermarkImage(dialog.pick_file_on_worker())
            }));
        } else if let Err(error) = self.platform.open_watermark_image() {
            self.report_error(ErrorKind::Watermark, error);
        }
    }

    /// Imports the image at `source` as the watermark and turns it on.
    pub(in crate::app) fn import_watermark_image(&mut self, source: PathBuf) {
        let destination = self.watermark_path();
        self.start_watermark_load(WatermarkSource::Chosen, move || {
            import_watermark(&source, destination.as_deref())
        });
    }

    /// Imports the image a platform picker copied into `folder`, then
    /// deletes the folder.
    pub(in crate::app) fn import_staged_watermark_image(&mut self, folder: PathBuf) {
        let destination = self.watermark_path();
        self.start_watermark_load(WatermarkSource::Chosen, move || {
            let imported = staged_image(&folder)
                .and_then(|source| import_watermark(&source, destination.as_deref()));
            if let Err(error) = std::fs::remove_dir_all(&folder) {
                log::warn!("could not delete the watermark import folder: {error}");
            }
            imported
        });
    }

    /// Runs `load` on a worker; [`Self::poll_watermark`] installs its result.
    fn start_watermark_load(
        &mut self,
        source: WatermarkSource,
        load: impl FnOnce() -> WatermarkLoad + Send + 'static,
    ) {
        self.export.watermark.loading = Some(PendingWatermarkLoad {
            receiver: spawn_ui_worker(&self.egui_ctx, load),
            source,
        });
    }

    /// Installs a finished image load and saves a changed placement once
    /// the pointer is released, so a slider drag is not saved every frame.
    pub(in crate::app) fn poll_watermark(&mut self) {
        let finished = self.export.watermark.loading.as_ref().and_then(|pending| {
            match pending.receiver.try_recv() {
                Ok(result) => Some((pending.source, result)),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some((
                    pending.source,
                    Err("the watermark worker stopped unexpectedly".to_owned()),
                )),
            }
        });
        if let Some((source, result)) = finished {
            self.export.watermark.loading = None;
            self.finish_watermark_load(source, result);
        }

        let watermark = &mut self.export.watermark;
        if watermark.placement_unsaved && !self.egui_ctx.input(|input| input.pointer.any_down()) {
            watermark.placement_unsaved = false;
            self.persist_performance_settings();
        }
    }

    fn finish_watermark_load(&mut self, source: WatermarkSource, result: WatermarkLoad) {
        match result {
            Ok(image) => {
                let texture = self.egui_ctx.load_texture(
                    "export-watermark",
                    egui::ColorImage::from_rgba_unmultiplied(
                        [image.width() as usize, image.height() as usize],
                        image.rgba(),
                    ),
                    egui::TextureOptions {
                        mipmap_mode: Some(egui::TextureFilter::Linear),
                        ..egui::TextureOptions::LINEAR
                    },
                );
                let watermark = &mut self.export.watermark;
                watermark.image = Some(image);
                watermark.texture = Some(texture);
                if source == WatermarkSource::Chosen {
                    watermark.enabled = true;
                    self.persist_performance_settings();
                }
            }
            Err(error) => {
                if source == WatermarkSource::Saved {
                    self.export.watermark.enabled = false;
                }
                self.report_error(
                    ErrorKind::Watermark,
                    format!("Could not load the watermark image: {error}"),
                );
            }
        }
    }

    fn watermark_path(&self) -> Option<PathBuf> {
        self.preferences
            .performance_settings_path
            .as_deref()
            .map(|settings| settings.with_file_name(WATERMARK_FILE_NAME))
    }
}

/// The file a platform picker staged in `folder`.
fn staged_image(folder: &Path) -> Result<PathBuf, String> {
    std::fs::read_dir(folder)
        .map_err(|error| format!("could not read the picked image: {error}"))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .find(|path| path.is_file())
        .ok_or_else(|| "the picked image could not be read".to_owned())
}

fn read_watermark(path: &Path) -> WatermarkLoad {
    let size = std::fs::metadata(path)
        .map_err(|error| format!("{}: {error}", path.display()))?
        .len();
    if size > MAX_WATERMARK_FILE_BYTES {
        return Err(format!(
            "the image is larger than {} MB",
            MAX_WATERMARK_FILE_BYTES / (1024 * 1024)
        ));
    }
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    WatermarkImage::decode(&bytes)
        .map(Arc::new)
        .map_err(|error| format!("{error:#}"))
}

/// Reads `source` and keeps it at `destination` as PNG, so the image
/// survives the original being moved or its access being revoked.
fn import_watermark(source: &Path, destination: Option<&Path>) -> WatermarkLoad {
    use image::ImageEncoder;
    let image = read_watermark(source)?;
    if let Some(destination) = destination {
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(
                image.rgba(),
                image.width(),
                image.height(),
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|error| format!("could not encode the watermark: {error}"))?;
        calibraw_core::file_ops::write_bytes_atomically(destination, &png)
            .map_err(|error| format!("could not save the watermark: {error}"))?;
    }
    Ok(image)
}
