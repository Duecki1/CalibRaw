//! Session-only eye controls. Saved edits, history, export and mask sources never
//! contain this state. Only the Develop preview reads projected copies.
use super::*;
use std::collections::HashSet;

#[derive(Clone, Default)]
pub(crate) struct PreviewVisibility {
    hidden: HashSet<(Option<usize>, &'static str)>,
    scope: Option<usize>,
    force_mask_overlay: bool,
    pending: bool,
    rebuild_required: bool,
    masks: Option<Arc<MaskStack>>,
    mask_names: Vec<String>,
}

impl PreviewVisibility {
    fn id() -> egui::Id {
        egui::Id::new("temporary-preview-card-visibility")
    }
    fn read(ctx: &egui::Context) -> Self {
        ctx.data(|data| data.get_temp::<Self>(Self::id()))
            .unwrap_or_default()
    }
    fn write(self, ctx: &egui::Context) {
        ctx.data_mut(|data| data.insert_temp(Self::id(), self));
    }
    pub(crate) fn clear(ctx: &egui::Context) {
        ctx.data_mut(|data| data.remove::<Self>(Self::id()));
    }
    pub(crate) fn invalidate_mask_cache(ctx: &egui::Context) {
        let mut state = Self::read(ctx);
        state.masks = None;
        state.write(ctx);
    }
    pub(crate) fn set_mask_scope(ctx: &egui::Context, scope: Option<usize>) {
        let mut state = Self::read(ctx);
        state.scope = scope;
        state.write(ctx);
    }
    pub(crate) fn current_scope(ctx: &egui::Context) -> Option<usize> {
        Self::read(ctx).scope
    }
    pub(crate) fn mask_overlay_forced(ctx: &egui::Context) -> bool {
        Self::read(ctx).force_mask_overlay
    }
    pub(crate) fn toggle_mask_overlay(ctx: &egui::Context) {
        let mut state = Self::read(ctx);
        state.force_mask_overlay = !state.force_mask_overlay;
        state.write(ctx);
        ctx.request_repaint();
    }
    pub(crate) fn visible(ctx: &egui::Context, title: &'static str) -> bool {
        let state = Self::read(ctx);
        !state.hidden.contains(&(state.scope, title))
    }
    /// Whether the photo-wide card `title` is shown, whichever mask is in scope.
    pub(in crate::app) fn globally_visible(ctx: &egui::Context, title: &'static str) -> bool {
        !Self::read(ctx).hidden.contains(&(None, title))
    }
    pub(crate) fn toggle(ctx: &egui::Context, title: &'static str) {
        let mut state = Self::read(ctx);
        let key = (state.scope, title);
        if !state.hidden.remove(&key) {
            state.hidden.insert(key);
        }
        state.pending = true;
        state.masks = None;
        state.write(ctx);
        ctx.request_repaint();
    }
    /// Re-renders the preview after an input of the projection other than
    /// the eye controls changed, such as a hovered preset.
    pub(in crate::app) fn refresh_projection(ctx: &egui::Context) {
        let mut state = Self::read(ctx);
        state.pending = true;
        state.masks = None;
        state.write(ctx);
        ctx.request_repaint();
    }
    pub(crate) fn show(ctx: &egui::Context, title: &'static str) {
        if !Self::visible(ctx, title) {
            Self::toggle(ctx, title);
        }
    }
    pub(in crate::app) fn requires_rebuild(ctx: &egui::Context) -> bool {
        Self::read(ctx).rebuild_required
    }
    pub(in crate::app) fn rebuilt(ctx: &egui::Context) {
        let mut state = Self::read(ctx);
        state.rebuild_required = false;
        state.write(ctx);
    }
    pub(crate) fn invalidate_masks(ctx: &egui::Context, masks: &MaskStack) {
        let mut state = Self::read(ctx);
        let names: Vec<_> = masks.masks.iter().map(|mask| mask.name.clone()).collect();
        if state.mask_names != names {
            // Structural edits must never transfer an eye state to a different mask.
            state.hidden.retain(|(scope, _)| scope.is_none());
            state.mask_names = names;
        }
        state.masks = None;
        state.write(ctx);
    }
    fn group(title: &str) -> Option<crate::pipeline::AdjustmentGroup> {
        use crate::pipeline::AdjustmentGroup;
        Some(match title {
            "Light" => AdjustmentGroup::Light,
            "Tone Curve" => AdjustmentGroup::ToneCurve,
            "Color" => AdjustmentGroup::Color,
            "Color Grading" => AdjustmentGroup::ColorGrading,
            "Detail" => AdjustmentGroup::Detail,
            "Effects" => AdjustmentGroup::Effects,
            "Color Mixer" => AdjustmentGroup::ColorMixer,
            _ => return None,
        })
    }
    fn exposure(&self, saved: ExposureParams) -> ExposureParams {
        let mut preview = saved;
        for (scope, title) in &self.hidden {
            if scope.is_none() {
                if *title == super::FILM_NEGATIVE_CARD_TITLE {
                    preview.film = crate::pipeline::FilmSource::Positive;
                } else if let Some(group) = Self::group(title) {
                    preview.reset_group(group);
                    if group == crate::pipeline::AdjustmentGroup::Detail {
                        preview.sharpen_amount = 0.0;
                    }
                }
            }
        }
        preview
    }
    fn project_masks(&self, saved: &MaskStack) -> MaskStack {
        let mut preview = saved.clone();
        for (scope, title) in &self.hidden {
            let Some(index) = *scope else {
                continue;
            };
            if *title == "Subject refinement" {
                preview.subject_refinement.clear();
            } else if let Some(mask) = preview.masks.get_mut(index) {
                if let Some(group) = Self::group(title) {
                    mask.adjustments.reset_group(group);
                } else {
                    mask.enabled = false;
                }
            }
        }
        preview
    }
}

impl CalibRawApp {
    pub(in crate::app) fn preview_exposure(&self) -> ExposureParams {
        let mut exposure = self.develop.exposure;
        if let Some(preset) = self.previewed_preset() {
            preset.preview_adjustments_on(&mut exposure);
        }
        let mut exposure = PreviewVisibility::read(&self.egui_ctx).exposure(exposure);
        exposure.point_color_visualize = (self.ui.sidebar_tab == SidebarTab::Adjustments
            && self.develop_ui.point_color_tab
            && self.develop_ui.point_color.visualize_range
            && !self.develop_ui.point_color.picker_active
            && self.develop_ui.point_color.selected < exposure.point_colors.len())
        .then_some(self.develop_ui.point_color.selected);
        exposure
    }
    pub(crate) fn preview_mask_stack(&self) -> Arc<MaskStack> {
        let mut state = PreviewVisibility::read(&self.egui_ctx);
        if let Some(masks) = state.masks {
            return masks;
        }
        let mut masks = match self.previewed_preset() {
            Some(preset) => {
                let mut with_preset = self.masks.stack.clone();
                preset.preview_masks_on(&mut with_preset);
                state.project_masks(&with_preset)
            }
            None => state.project_masks(&self.masks.stack),
        };
        if self.ui.sidebar_tab == SidebarTab::Masks
            && self.develop_ui.mask_point_color_tab
            && self.develop_ui.mask_point_color.visualize_range
            && !self.develop_ui.mask_point_color.picker_active
        {
            if let Some(mask) = masks.selected_mask_mut() {
                let selected = self.develop_ui.mask_point_color.selected;
                if selected < mask.adjustments.point_colors.len() {
                    mask.adjustments.point_color_visualize = Some(selected);
                }
            }
        }
        let masks = Arc::new(masks);
        state.masks = Some(Arc::clone(&masks));
        state.write(&self.egui_ctx);
        masks
    }
    pub(in crate::app) fn sync_preview_visibility(&mut self) {
        let mut state = PreviewVisibility::read(&self.egui_ctx);
        let exposure = self.preview_exposure();
        if !state.pending {
            // Range visualization is a preview aid, never an edit or history entry.
            if exposure.point_color_visualize != self.develop.target_exposure.point_color_visualize
            {
                self.develop.target_exposure = exposure;
                self.queue_preview_processing(ProcessingStage::Output);
            }
            return;
        }
        let source_changed =
            exposure.ai_denoise_enabled != self.develop.target_exposure.ai_denoise_enabled;
        state.pending = false;
        state.rebuild_required |= source_changed;
        state.write(&self.egui_ctx);
        self.develop.target_exposure = exposure;
        self.develop.target_look = self.preview_look();
        self.preview.original_rendered_state = None;
        self.masks.dirty_layers.fill(true);
        self.masks.detail_dirty_layers.fill(true);
        self.masks.navigation_dirty_layers.fill(true);
        if source_changed {
            self.preview.quality_dirty = true;
            self.preview.drop_detail_rebuild();
        }
        self.queue_preview_processing(ProcessingStage::Raw);
    }
}
