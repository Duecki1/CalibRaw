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
