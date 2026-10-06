//! Quick preview of a preset while the pointer rests on it in the Presets
//! panel. Only the Develop preview reads it, through the projection in
//! `preview_visibility`; saved edits, history and export never contain it.
use super::*;
use crate::app::preview_visibility::PreviewVisibility;

/// How long the pointer must rest on a preset before the preview renders.
/// Without a delay, moving across the list re-renders the photo once per row.
const HOVER_PREVIEW_DELAY: Duration = Duration::from_millis(150);

#[derive(Default)]
pub(crate) struct PresetHoverPreview {
    /// The preset under the pointer this frame. Written by the Presets panel
    /// and consumed every frame, so a hidden panel ends the preview.
    requested: Option<PathBuf>,
    /// The preset under the pointer and when the pointer reached it.
    pending: Option<(PathBuf, Instant)>,
    /// The preset rendered on the preview.
    shown: Option<PathBuf>,
    /// A preset that was just applied. It is not previewed again until the
    /// pointer leaves it; previewing it on top of itself would add its masks
    /// a second time.
    suppressed: Option<PathBuf>,
}

impl PresetHoverPreview {
    /// Reports the preset under the pointer for this frame.
    #[cfg(not(target_os = "android"))]
    pub(crate) fn request(&mut self, path: PathBuf) {
        self.requested = Some(path);
    }

    /// Ends the preview of `path` because it was just applied. Returns
    /// whether a preview was on screen.
    fn suppress(&mut self, path: &Path) -> bool {
        self.suppressed = Some(path.to_owned());
        self.pending = None;
        self.shown.take().is_some()
    }

    /// Advances the hover delay. Returns whether the shown preset changed,
    /// and how long to wait before the pending preset is due.
    fn advance(&mut self, now: Instant) -> (bool, Option<Duration>) {
        let requested = self.requested.take();
        if self.suppressed.is_some() && requested != self.suppressed {
            self.suppressed = None;
        }
        let requested = requested.filter(|path| Some(path) != self.suppressed.as_ref());

        let mut wait = None;
        let next = match requested {
            None => {
                self.pending = None;
                None
            }
            Some(path) => {
                let (pending_path, since) = match &self.pending {
                    Some((pending_path, since)) if *pending_path == path => {
                        (pending_path.clone(), *since)
                    }
                    _ => {
                        self.pending = Some((path.clone(), now));
                        (path, now)
                    }
                };
                let waited = now.saturating_duration_since(since);
                if waited >= HOVER_PREVIEW_DELAY {
                    Some(pending_path)
                } else {
                    wait = Some(HOVER_PREVIEW_DELAY - waited);
                    // Keep the current preview until the next one is due, so
                    // moving between rows does not flash the unedited photo.
                    self.shown.clone()
                }
            }
        };
        let changed = next != self.shown;
        self.shown = next;
        (changed, wait)
    }
}

impl CalibRawApp {
    /// The preset rendered on the Develop preview without being applied.
    pub(in crate::app) fn previewed_preset(&self) -> Option<&Preset> {
        self.presets
            .hover
            .shown
            .as_deref()
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

#[cfg(all(test, not(target_os = "android")))]
mod tests {
    use super::*;

    fn path(name: &str) -> PathBuf {
        PathBuf::from(format!("/presets/{name}.calibraw-preset"))
    }

    #[test]
    fn previews_start_after_the_pointer_rests() {
        let start = Instant::now();
        let mut hover = PresetHoverPreview::default();

        hover.request(path("a"));
        let (changed, wait) = hover.advance(start);
        assert!(!changed);
        assert_eq!(wait, Some(HOVER_PREVIEW_DELAY));

        hover.request(path("a"));
        assert_eq!(hover.advance(start + HOVER_PREVIEW_DELAY), (true, None));
        assert_eq!(hover.shown, Some(path("a")));
    }

    #[test]
    fn moving_to_another_preset_keeps_the_current_preview_until_the_next_is_due() {
        let start = Instant::now();
        let mut hover = PresetHoverPreview::default();
        hover.request(path("a"));
        hover.advance(start);
        hover.request(path("a"));
        hover.advance(start + HOVER_PREVIEW_DELAY);

        let moved = start + HOVER_PREVIEW_DELAY * 2;
        hover.request(path("b"));
        assert!(!hover.advance(moved).0);
        assert_eq!(hover.shown, Some(path("a")));
        hover.request(path("b"));
        assert!(hover.advance(moved + HOVER_PREVIEW_DELAY).0);
        assert_eq!(hover.shown, Some(path("b")));
    }

    #[test]
    fn leaving_the_list_ends_the_preview_at_once() {
        let start = Instant::now();
        let mut hover = PresetHoverPreview::default();
        hover.request(path("a"));
        hover.advance(start);
        hover.request(path("a"));
        hover.advance(start + HOVER_PREVIEW_DELAY);

        assert_eq!(hover.advance(start + HOVER_PREVIEW_DELAY * 2), (true, None));
        assert_eq!(hover.shown, None);
    }

    #[test]
    fn an_applied_preset_is_not_previewed_again_until_the_pointer_leaves_it() {
        let start = Instant::now();
        let mut hover = PresetHoverPreview::default();
        hover.request(path("a"));
        hover.advance(start);
        hover.request(path("a"));
        hover.advance(start + HOVER_PREVIEW_DELAY);
        assert!(hover.suppress(&path("a")));

        let later = start + HOVER_PREVIEW_DELAY * 4;
        hover.request(path("a"));
        assert_eq!(hover.advance(later), (false, None));
        assert_eq!(hover.shown, None);

        // Leaving and coming back previews it again.
        hover.advance(later);
        hover.request(path("a"));
        hover.advance(later);
        hover.request(path("a"));
        assert!(hover.advance(later + HOVER_PREVIEW_DELAY).0);
    }

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

    #[test]
    fn hover_previews_show_the_presets_colour_look_without_saving_it() {
        let folder = std::env::temp_dir().join(format!(
            "calibraw-preset-hover-look-{}-{}",
            std::process::id(),
            start_nanos()
        ));
        let look = crate::pipeline::ColorLutEdit::new(
            "Film",
            crate::pipeline::ColorLut::from_function(2, |rgb| rgb.map(|v| v * 0.5)).unwrap(),
        );
        let mut source = crate::sidecar::default_edit_state();
        source.color_lut = Some(look.clone());
        let selection = EditSelection {
            color_lut: true,
            ..EditSelection::default()
        };
        let preset = Preset::new("Film", "", selection, &source).unwrap();
        let preset_path = crate::presets::save_new_preset(&folder, &preset).unwrap();

        let ctx = egui::Context::default();
        let mut app = CalibRawApp::empty(&ctx);
        app.presets = PresetState::load(Some(folder.clone()));
        app.reset_edit_history();
        let saved = app.capture_sidecar_edit_state();
        assert!(app.preview_color_lut().is_none());

        let start = Instant::now();
        app.presets.hover.request(preset_path.clone());
        app.sync_preset_hover_preview_at(start);
        app.presets.hover.request(preset_path);
        app.sync_preset_hover_preview_at(start + HOVER_PREVIEW_DELAY);
        assert_eq!(app.preview_color_lut(), Some(look));
        assert!(app.develop.color_lut.is_none());
        assert_eq!(app.capture_sidecar_edit_state(), saved);

        // When the preview ends, the photo's own look is back.
        app.develop.color_lut = Some(crate::pipeline::ColorLutEdit::new(
            "Own",
            crate::pipeline::ColorLut::from_function(2, |rgb| rgb).unwrap(),
        ));
        assert!(app.preview_color_lut().is_some());

        app.sync_preset_hover_preview_at(start + HOVER_PREVIEW_DELAY * 2);
        assert_eq!(
            app.preview_color_lut().map(|look| look.name),
            Some("Own".to_owned())
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
