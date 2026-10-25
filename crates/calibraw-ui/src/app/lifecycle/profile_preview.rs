//! Quick preview of a camera profile while the pointer rests on it in the
//! Camera profile card. The previews render the colour the profile gives the
//! open photo, resolved from the decode in memory
//! (`LoadedRaw::profile_color_for_selection`); saved edits, history and export
//! never contain it. Choosing the profile switches the document to it without
//! decoding again (`DecodedRaw::original_for`), and its preview stays until
//! the document has it, so the photo does not flash back meanwhile.

use super::document_load::selection_decode_request;
use super::*;
use crate::app::hover_preview::HoverPreview;
use crate::pipeline::{ProfileColor, PREVIEWED_PROFILE_BYTES};

/// Longer than the other hover previews: a profile renders again from the
/// RAW stage.
const PROFILE_HOVER_DELAY: Duration = Duration::from_millis(250);

/// A row of the camera profile dropdown: `None` for automatic selection, the
/// profile folder for the embedded matrix, or a DCP file.
pub(crate) type CameraProfileChoice = Option<PathBuf>;

pub(crate) struct CameraProfilePreview {
    /// The row of the Camera profile card the pointer rests on.
    pub(crate) hover: HoverPreview<CameraProfileChoice>,
    /// The colour of a hovered row being resolved; `None` when it cannot be
    /// previewed.
    loading: Option<(
        CameraProfileChoice,
        mpsc::Receiver<Option<Arc<ProfileColor>>>,
    )>,
    /// The colour of the latest hovered row that can be previewed. It stays
    /// while the next one resolves.
    shown: Option<Arc<ProfileColor>>,
    /// The document a chosen, previewed profile is replacing; its preview
    /// stays until the switch ends.
    switching_from: Option<u64>,
}

impl Default for CameraProfilePreview {
    fn default() -> Self {
        Self {
            hover: HoverPreview::with_delay(PROFILE_HOVER_DELAY),
            loading: None,
            shown: None,
            switching_from: None,
        }
    }
}

/// Whether the graphs that show `raw` reserve room for a previewed camera
/// profile: the Camera profile card lists profiles to rest the pointer on.
pub(in crate::app) fn reserves_profile_preview(raw: &LoadedRaw) -> bool {
    !raw.available_camera_profiles.is_empty()
}

/// The colour `request` gives `raw`, when the previews can render it.
fn previewable_color(raw: &LoadedRaw, request: &DecodeRequest) -> Option<Arc<ProfileColor>> {
    let color = match raw.profile_color_for_selection(
        request.mode,
        request.folder.as_deref(),
        request.profile.as_deref(),
    )? {
        Ok(color) => color,
        Err(error) => {
            log::warn!("could not preview the camera profile: {error:#}");
            return None;
        }
    };
    let bytes = color.camera_profile.gpu_data().words.len() * std::mem::size_of::<[f32; 4]>();
    (bytes as u64 <= PREVIEWED_PROFILE_BYTES).then_some(color)
}

impl CalibRawApp {
    /// The camera profile colour the shown previews of `raw`, a view of the
    /// open photo, render: a previewed profile's, otherwise `raw`'s own.
    pub(in crate::app) fn preview_profile_color<'a>(
        &'a self,
        raw: &'a LoadedRaw,
    ) -> &'a Arc<ProfileColor> {
        self.develop
            .target_profile_color
            .as_ref()
            .unwrap_or(&raw.profile_color)
    }

    /// Whether the open document is being reopened, as for a chosen camera
    /// profile.
    fn document_switch_pending(&self) -> bool {
        self.develop.load_receiver.is_some()
            || self.android.picker_pending
            || self.android.pending_android_profile_reload.is_some()
    }

    /// Starts, switches or ends the camera profile preview. Runs once per
    /// frame after the Camera profile card has reported the row under the
    /// pointer.
    pub(in crate::app) fn sync_camera_profile_preview(&mut self) {
        let generation = self.persistence.document_generation;
        if self
            .develop
            .profile_preview
            .switching_from
            .is_some_and(|from| from != generation || !self.document_switch_pending())
        {
            let preview = &mut self.develop.profile_preview;
            preview.switching_from = None;
            preview.shown = None;
        }

        let (changed, wait) = self.develop.profile_preview.hover.advance(Instant::now());
        if let Some(wait) = wait {
            self.egui_ctx.request_repaint_after(wait);
        }
        if changed {
            match self.develop.profile_preview.hover.shown().cloned() {
                Some(choice) => self.resolve_previewed_profile(choice),
                None => {
                    let preview = &mut self.develop.profile_preview;
                    preview.loading = None;
                    if preview.switching_from.is_none() {
                        preview.shown = None;
                    }
                }
            }
        }
        self.poll_previewed_profile();

        let preview = &self.develop.profile_preview;
        let target = (preview.hover.shown().is_some() || preview.switching_from.is_some())
            .then(|| preview.shown.clone())
            .flatten();
        let same = match (&target, &self.develop.target_profile_color) {
            (Some(target), Some(current)) => Arc::ptr_eq(target, current),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.develop.target_profile_color = target;
            self.queue_preview_processing(ProcessingStage::Raw);
        }
    }

    fn resolve_previewed_profile(&mut self, choice: CameraProfileChoice) {
        let preview = &mut self.develop.profile_preview;
        if choice == self.develop.selected_camera_profile {
            // The photo shows this profile already.
            preview.loading = None;
            preview.shown = None;
            return;
        }
        let Some(raw) = self.develop.original_raw.clone() else {
            return;
        };
        let request = selection_decode_request(choice.clone(), &self.camera_profile_settings());
        let receiver = spawn_ui_worker(&self.egui_ctx, move || previewable_color(&raw, &request));
        self.develop.profile_preview.loading = Some((choice, receiver));
    }

    fn poll_previewed_profile(&mut self) {
        let preview = &mut self.develop.profile_preview;
        let Some((choice, receiver)) = &preview.loading else {
            return;
        };
        let color = match receiver.try_recv() {
            Ok(color) => color,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => None,
        };
        if preview.hover.shown() == Some(choice) {
            preview.shown = color;
        }
        preview.loading = None;
    }

    /// Ends the preview of `choice`, which was just chosen. A shown preview
    /// stays until the document has switched to it.
    pub(in crate::app) fn keep_camera_profile_preview_for(&mut self, choice: &CameraProfileChoice) {
        let generation = self.persistence.document_generation;
        let preview = &mut self.develop.profile_preview;
        let previewing = preview.hover.shown() == Some(choice) && preview.shown.is_some();
        preview.hover.suppress(choice);
        preview.loading = None;
        if previewing {
            preview.switching_from = Some(generation);
        } else {
            preview.shown = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_previewing(choice: &CameraProfileChoice) -> (CalibRawApp, Arc<ProfileColor>) {
        let mut app = CalibRawApp::empty(&egui::Context::default());
        let color = Arc::new(ProfileColor::default());
        let preview = &mut app.develop.profile_preview;
        preview.hover = HoverPreview::with_delay(Duration::ZERO);
        preview.hover.request(choice.clone());
        preview.hover.advance(Instant::now());
        // As a resolved row of a photo with a colour source would leave it.
        preview.shown = Some(Arc::clone(&color));
        (app, color)
    }

    #[test]
    fn a_hovered_profile_renders_until_the_pointer_leaves() {
        let choice = Some(PathBuf::from("/profiles/Vivid.dcp"));
        let (mut app, color) = app_previewing(&choice);
        app.develop.profile_preview.hover.request(choice);
        app.sync_camera_profile_preview();
        let target = app.develop.target_profile_color.as_ref().unwrap();
        assert!(Arc::ptr_eq(target, &color));

        app.sync_camera_profile_preview();
        assert!(app.develop.target_profile_color.is_none());
    }

    #[test]
    fn a_chosen_profile_stays_previewed_until_the_document_switches() {
        let choice = Some(PathBuf::from("/profiles/Vivid.dcp"));
        let (mut app, color) = app_previewing(&choice);
        app.develop.profile_preview.hover.request(choice.clone());
        app.sync_camera_profile_preview();

        app.keep_camera_profile_preview_for(&choice);
        let (_sender, receiver) = mpsc::channel();
        app.develop.load_receiver = Some(receiver);
        app.sync_camera_profile_preview();
        let target = app.develop.target_profile_color.as_ref().unwrap();
        assert!(Arc::ptr_eq(target, &color));

        // The reloaded document shows the profile itself.
        app.persistence.document_generation += 1;
        app.sync_camera_profile_preview();
        assert!(app.develop.target_profile_color.is_none());
    }

    #[test]
    fn a_failed_switch_ends_the_preview() {
        let choice = Some(PathBuf::from("/profiles/Vivid.dcp"));
        let (mut app, _) = app_previewing(&choice);
        app.develop.profile_preview.hover.request(choice.clone());
        app.sync_camera_profile_preview();
        app.keep_camera_profile_preview_for(&choice);
        // No reload started, as when reopening the photo failed.
        app.sync_camera_profile_preview();
        assert!(app.develop.target_profile_color.is_none());
    }
}
