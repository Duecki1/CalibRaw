//! What the Develop preview renders of the open edit: its saved card
//! switches applied (`card_switches` in calibraw-core), as every renderer
//! applies them, plus the preview-only aids on top: a hovered preset, the
//! Point Color range visualization and the forced mask overlay.
use super::*;
use crate::pipeline::{AdjustmentGroupSet, CardSwitch, CardSwitches};

/// The card switches of the open edit, to notice when one changes: by a
/// switch, a reset, an undo or a paste.
#[derive(Clone, Default, PartialEq)]
struct SwitchState {
    develop: CardSwitches,
    lens: bool,
    masks: Vec<AdjustmentGroupSet>,
    subject_refinement: bool,
}

#[derive(Clone, Default)]
pub(crate) struct PreviewVisibility {
    scope: Option<usize>,
    force_mask_overlay: bool,
    pending: bool,
    rebuild_required: bool,
    masks: Option<Arc<MaskStack>>,
    /// `None` until the open edit's switches are first seen, which needs no
    /// render of its own.
    switches: Option<SwitchState>,
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
    /// The mask whose cards the sidebar draws, which their footer actions
    /// on portrait screens belong to; `None` for the photo-wide cards.
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
    /// Re-renders the preview after an input of the projection other than
    /// the card switches changed, such as a hovered preset.
    pub(in crate::app) fn refresh_projection(ctx: &egui::Context) {
        let mut state = Self::read(ctx);
        state.pending = true;
        state.masks = None;
        state.write(ctx);
        ctx.request_repaint();
    }
    pub(in crate::app) fn requires_rebuild(ctx: &egui::Context) -> bool {
        Self::read(ctx).rebuild_required
    }
    pub(in crate::app) fn rebuilt(ctx: &egui::Context) {
        let mut state = Self::read(ctx);
        state.rebuild_required = false;
        state.write(ctx);
    }
    /// Forgets what the preview knew of the previous edit once another is
    /// installed and rendered, such as an opened photo's.
    pub(in crate::app) fn edit_installed(ctx: &egui::Context) {
        let mut state = Self::read(ctx);
        state.masks = None;
        state.switches = None;
        state.write(ctx);
    }
}

impl CalibRawApp {
    /// Switches the photo-wide `card` on or off. Like any edit it is saved
    /// and an undo step; the preview follows (`sync_preview_visibility`).
    pub(crate) fn set_card_on(&mut self, card: CardSwitch, on: bool) {
        if self.develop.exposure.set_card_on(card, on) {
            self.mark_pipeline_dirty();
        }
    }

    /// The open edit's masks as they render: their switched-off cards at
    /// rest, without the preview's aids.
    pub(in crate::app) fn applied_masks(&self) -> MaskStack {
        let mut masks = self.masks.stack.clone();
        masks.apply_card_switches();
        masks
    }

    fn card_switch_state(&self) -> SwitchState {
        SwitchState {
            develop: self.develop.exposure.cards_off,
            lens: self.develop.lens_correction.enabled,
            masks: self
                .masks
                .stack
                .masks
                .iter()
                .map(|mask| mask.cards_off)
                .collect(),
            subject_refinement: self.masks.stack.subject_refinement.enabled,
        }
    }

    pub(in crate::app) fn preview_exposure(&self) -> ExposureParams {
        let mut exposure = self.develop.exposure;
        if let Some(preset) = self.previewed_preset() {
            preset.preview_adjustments_on(&mut exposure);
        }
        let mut exposure = exposure.applied(self.develop.lens_correction.enabled);
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
        let mut masks = self.masks.stack.clone();
        if let Some(preset) = self.previewed_preset() {
            preset.preview_masks_on(&mut masks);
        }
        masks.apply_card_switches();
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
        let switches = self.card_switch_state();
        match &state.switches {
            Some(seen) if *seen == switches => {}
            seen => {
                // A switch reaches far: AI denoise, the film negative and
                // mask layers. The first look at an edit renders nothing.
                state.pending |= seen.is_some();
                state.masks = None;
                state.switches = Some(switches);
                state.clone().write(&self.egui_ctx);
            }
        }
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
