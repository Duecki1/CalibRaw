//! Quick preview of a preset while the pointer rests on it in the Presets
//! panel. Only the Develop preview reads it, through the projection in
//! `preview_visibility`; saved edits, history and export never contain it.
use super::*;
use crate::app::hover_preview::HoverPreview;
use crate::app::preview_visibility::PreviewVisibility;

/// The preset rendered on the preview, keyed by its file.
pub(crate) type PresetHoverPreview = HoverPreview<PathBuf>;

impl CalibRawApp {
    /// The preset rendered on the Develop preview without being applied.
    pub(in crate::app) fn previewed_preset(&self) -> Option<&Preset> {
        self.presets
            .hover
            .shown()
            .and_then(|path| self.presets.get(path))
    }

    /// Starts, switches or ends the hover preview. Runs once per frame after
    /// the Presets panel has reported the preset under the pointer.
    pub(in crate::app) fn sync_preset_hover_preview(&mut self) {
        self.sync_preset_hover_preview_at(Instant::now());
    }

    fn sync_preset_hover_preview_at(&mut self, now: Instant) {
        let (changed, wait) = self.presets.hover.advance(now);
        if let Some(wait) = wait {
            self.egui_ctx.request_repaint_after(wait);
        }
        if changed {
            PreviewVisibility::refresh_projection(&self.egui_ctx);
        }
    }

    pub(super) fn end_preset_hover_preview_for(&mut self, path: &Path) {
        if self.presets.hover.suppress(path) {
            PreviewVisibility::refresh_projection(&self.egui_ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::hover_preview::HOVER_PREVIEW_DELAY;

    #[test]
    fn hover_previews_render_without_touching_saved_edits_or_history() {
        let folder = std::env::temp_dir().join(format!(
            "calibraw-preset-hover-{}-{}",
            std::process::id(),
            start_nanos()
        ));
        let mut source = crate::sidecar::default_edit_state();
        source.exposure.exposure = 1.25;
        let selection = EditSelection {
            adjustment_groups: [crate::pipeline::AdjustmentGroup::Light]
                .into_iter()
                .collect(),
            ..EditSelection::default()
        };
        let preset = Preset::new("Bright", "", selection, &source).unwrap();
        let preset_path = crate::presets::save_new_preset(&folder, &preset).unwrap();

        let ctx = egui::Context::default();
        let mut app = CalibRawApp::empty(&ctx);
        app.presets = PresetState::load(Some(folder.clone()));
        app.reset_edit_history();
        let saved = app.capture_sidecar_edit_state();
        let revision = app.edit_commit_revision();

        let start = Instant::now();
        app.presets.hover.request(preset_path.clone());
        app.sync_preset_hover_preview_at(start);
        assert_eq!(app.preview_exposure().exposure, saved.exposure.exposure);
        app.presets.hover.request(preset_path);
        app.sync_preset_hover_preview_at(start + HOVER_PREVIEW_DELAY);
        app.sync_preview_visibility();

        assert_eq!(app.preview_exposure().exposure, 1.25);
        assert_eq!(app.develop.target_exposure.exposure, 1.25);
        assert_eq!(app.capture_sidecar_edit_state(), saved);
        app.observe_edit_history(&ctx);
        assert_eq!(app.edit_commit_revision(), revision);

        // Without a request from the panel, the next frame ends the preview.
        app.sync_preset_hover_preview_at(start + HOVER_PREVIEW_DELAY * 2);
        app.sync_preview_visibility();
        assert_eq!(
            app.develop.target_exposure.exposure,
            saved.exposure.exposure
        );

        std::fs::remove_dir_all(folder).unwrap();
    }

    fn start_nanos() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }
}
