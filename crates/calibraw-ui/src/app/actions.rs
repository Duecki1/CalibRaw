use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AppAction {
    UndoEdit,
    RedoEdit,
    SaveEdits,
    SelectSidebarTab(SidebarTab),
    SelectInpaintTool(InpaintTool),
}

impl CalibRawApp {
    pub(crate) fn action_enabled(&self, action: AppAction) -> bool {
        match action {
            AppAction::UndoEdit => self.can_undo_edit(),
            AppAction::RedoEdit => self.can_redo_edit(),
            AppAction::SaveEdits => self.can_save_edits(),
            AppAction::SelectSidebarTab(_) | AppAction::SelectInpaintTool(_) => true,
        }
    }

    pub(crate) fn dispatch_action(&mut self, action: AppAction) -> bool {
        if !self.action_enabled(action) {
            return false;
        }
        match action {
            AppAction::UndoEdit => self.undo_edit(),
            AppAction::RedoEdit => self.redo_edit(),
            AppAction::SaveEdits => self.save_edits_now(),
            AppAction::SelectSidebarTab(tab) => self.select_sidebar_tab_action(tab),
            AppAction::SelectInpaintTool(tool) => self.select_inpaint_tool_action(tool),
        }
        true
    }

    fn shortcut_overlays_clear(&self, ctx: &egui::Context) -> bool {
        !egui::Popup::is_any_open(ctx)
            && ctx.memory(|memory| memory.top_modal_layer().is_none())
            && !self.transient_ui_open(ctx)
    }

    pub(crate) fn app_shortcuts_allowed(&self, ctx: &egui::Context) -> bool {
        !ctx.egui_wants_keyboard_input() && self.shortcut_overlays_clear(ctx)
    }

    pub(crate) fn edit_history_shortcuts_allowed(&self, ctx: &egui::Context) -> bool {
        self.shortcut_overlays_clear(ctx)
    }

    #[cfg(not(target_os = "android"))]
    pub(crate) fn navigation_shortcuts_allowed(&self, ctx: &egui::Context) -> bool {
        self.app_shortcuts_allowed(ctx) && ctx.memory(|memory| memory.focused().is_none())
    }

    fn transient_ui_open(&self, ctx: &egui::Context) -> bool {
        self.ui.onboarding_step.is_some()
            || self.ui.unsupported_file_dialog.is_some()
            || self.ui.version_check.dialog_open()
            || self.library.transient_dialog_open()
            || self.presets.dialog_open()
            || self.ai.consent.is_some()
            || self.ai.object_error_dialog.is_some()
            || self.persistence.sidecar_save_error_dialog.is_some()
            || self.ui.gpu_memory_error_dialog
            || self.foreground_operation.is_some()
            || self.ai.library_mask_refresh.is_some()
            || self
                .export
                .task
                .as_ref()
                .is_some_and(|task| !task.minimized)
            || crate::ui::sidebar::Sidebar::mask_dialog_open(ctx)
    }

    fn select_sidebar_tab_action(&mut self, tab: SidebarTab) {
        let previous = self.ui.sidebar_tab;
        if previous == tab {
            return;
        }
        self.ui.sidebar_tab = tab;
        if previous == SidebarTab::Crop && tab != SidebarTab::Crop {
            self.develop_ui.crop_drag = None;
            self.develop_ui.straighten_tool_active = false;
            self.develop_ui.straighten_drag = None;
        }
        if tab != SidebarTab::Adjustments {
            self.develop_ui.cancel_point_color_preview();
            self.develop_ui.cancel_white_balance_picker();
        }
        if tab != SidebarTab::Masks {
            self.cancel_mask_point_color_preview();
        }
        self.sync_ai_runtime();
    }

    /// Stops picking point colors for the selected mask and hides their range
    /// overlay. The overlay is baked into the cached mask preview, so hiding it
    /// requires a refresh.
    pub(crate) fn cancel_mask_point_color_preview(&mut self) {
        let was_visualizing = self.develop_ui.mask_point_color.visualize_range;
        self.develop_ui.mask_point_color.picker_active = false;
        self.develop_ui.mask_point_color.visualize_range = false;
        if was_visualizing {
            self.refresh_mask_overlay_preview();
        }
    }

    pub(crate) fn refresh_mask_overlay_preview(&mut self) {
        super::preview_visibility::PreviewVisibility::invalidate_mask_cache(&self.egui_ctx);
        self.queue_preview_processing(crate::pipeline::ProcessingStage::Output);
    }

    fn select_inpaint_tool_action(&mut self, tool: InpaintTool) {
        if self.inpaint.tool == tool {
            return;
        }
        self.finish_inpaint_stroke_opacity_edit();
        self.inpaint.tool = tool;
        self.inpaint.active_points.clear();
        self.inpaint.last_brush_uv = None;
        self.inpaint.source_pick_active = false;
        self.inpaint.source_placement_press = false;
        self.inpaint.aligned_offset = None;
        self.inpaint.hovered_stroke = None;
        self.inpaint.selected_stroke = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_actions_do_not_dispatch() {
        let ctx = egui::Context::default();
        crate::ui::theme::install(&ctx);
        let mut app = CalibRawApp::empty(&ctx);

        assert!(!app.action_enabled(AppAction::UndoEdit));
        assert!(!app.dispatch_action(AppAction::UndoEdit));
        assert!(!app.action_enabled(AppAction::SaveEdits));
        assert!(!app.dispatch_action(AppAction::SaveEdits));
    }

    #[test]
    fn transient_ui_blocks_app_shortcuts() {
        let ctx = egui::Context::default();
        crate::ui::theme::install(&ctx);
        let mut app = CalibRawApp::empty(&ctx);

        app.ui.onboarding_step = Some(OnboardingStep::Appearance);
        assert!(!app.app_shortcuts_allowed(&ctx));
        app.ui.onboarding_step = None;
        assert!(app.app_shortcuts_allowed(&ctx));
        app.ai.consent = Some(AiConsent {
            feature: calibraw_ai::AiFeature::Subject,
            runtime_download_needed: false,
            origin: AiJobOrigin::Requested,
        });
        assert!(!app.app_shortcuts_allowed(&ctx));
        app.ai.consent = None;

        egui::Popup::open_id(&ctx, egui::Id::new("shortcut-blocking-test-popup"));
        assert!(!app.app_shortcuts_allowed(&ctx));
        egui::Popup::close_all(&ctx);
    }

    #[test]
    fn focused_widgets_block_image_navigation_shortcuts() {
        let ctx = egui::Context::default();
        crate::ui::theme::install(&ctx);
        let mut app = CalibRawApp::empty(&ctx);
        app.ui.onboarding_step = None;
        let focus_id = egui::Id::new("navigation-focus-test");

        assert!(app.navigation_shortcuts_allowed(&ctx));
        ctx.memory_mut(|memory| memory.request_focus(focus_id));
        assert!(!app.navigation_shortcuts_allowed(&ctx));
    }

    #[test]
    fn focused_widgets_do_not_block_develop_edit_history_shortcuts() {
        let ctx = egui::Context::default();
        crate::ui::theme::install(&ctx);
        let mut app = CalibRawApp::empty(&ctx);
        app.ui.onboarding_step = None;
        let focus_id = egui::Id::new("edit-history-focus-test");

        ctx.memory_mut(|memory| memory.request_focus(focus_id));
        assert!(app.edit_history_shortcuts_allowed(&ctx));

        egui::Popup::open_id(&ctx, egui::Id::new("edit-history-popup-test"));
        assert!(!app.edit_history_shortcuts_allowed(&ctx));
        egui::Popup::close_all(&ctx);
    }

    #[test]
    fn selecting_sidebar_tab_clears_transient_tools() {
        let ctx = egui::Context::default();
        crate::ui::theme::install(&ctx);
        let mut app = CalibRawApp::empty(&ctx);
        app.ui.sidebar_tab = SidebarTab::Crop;
        app.develop_ui.straighten_tool_active = true;
        app.develop_ui.crop_drag = Some(CropDragState {
            handle: CropHandle::Move,
            start: [0.0, 0.0],
            crop: [0.0, 0.0, 1.0, 1.0],
        });

        assert!(app.dispatch_action(AppAction::SelectSidebarTab(SidebarTab::Masks)));
        assert_eq!(app.ui.sidebar_tab, SidebarTab::Masks);
        assert!(!app.develop_ui.straighten_tool_active);
        assert!(app.develop_ui.crop_drag.is_none());
    }

    #[test]
    fn reselecting_tab_is_a_noop() {
        let ctx = egui::Context::default();
        crate::ui::theme::install(&ctx);
        let mut app = CalibRawApp::empty(&ctx);
        app.ui.active_tab = AppTab::Develop;
        app.ui.thumbnail_cache_size = Some(Ok(42));
        app.preview.original_requested = true;

        app.activate_tab(AppTab::Develop);

        assert_eq!(app.ui.thumbnail_cache_size, Some(Ok(42)));
        assert!(app.preview.original_requested);
    }

    #[test]
    fn entering_settings_clears_thumbnail_cache_once() {
        let ctx = egui::Context::default();
        crate::ui::theme::install(&ctx);
        let mut app = CalibRawApp::empty(&ctx);
        app.ui.thumbnail_cache_size = Some(Ok(42));

        app.activate_tab(AppTab::Settings);
        assert!(app.ui.thumbnail_cache_size.is_none());

        app.ui.thumbnail_cache_size = Some(Ok(7));
        app.activate_tab(AppTab::Settings);
        assert_eq!(app.ui.thumbnail_cache_size, Some(Ok(7)));
    }

    #[test]
    fn leaving_develop_clears_original_preview_request() {
        let ctx = egui::Context::default();
        crate::ui::theme::install(&ctx);
        let mut app = CalibRawApp::empty(&ctx);
        app.ui.active_tab = AppTab::Develop;
        app.preview.original_requested = true;

        app.activate_tab(AppTab::Library);

        assert!(!app.preview.original_requested);
    }

    #[test]
    fn selecting_current_sidebar_tab_preserves_white_balance_picker() {
        let ctx = egui::Context::default();
        crate::ui::theme::install(&ctx);
        let mut app = CalibRawApp::empty(&ctx);
        app.ui.sidebar_tab = SidebarTab::Adjustments;
        app.develop_ui.white_balance_picker_active = true;
        app.develop_ui.white_balance_picker_drag = Some([[0.1, 0.2], [0.3, 0.4]]);

        app.dispatch_action(AppAction::SelectSidebarTab(SidebarTab::Adjustments));

        assert!(app.develop_ui.white_balance_picker_active);
        assert_eq!(
            app.develop_ui.white_balance_picker_drag,
            Some([[0.1, 0.2], [0.3, 0.4]])
        );
    }

    #[test]
    fn selecting_current_inpaint_tool_preserves_interaction_state() {
        let ctx = egui::Context::default();
        crate::ui::theme::install(&ctx);
        let mut app = CalibRawApp::empty(&ctx);
        app.inpaint.tool = InpaintTool::Remove;
        app.inpaint.source_pick_active = true;
        app.inpaint.hovered_stroke = Some(2);

        app.dispatch_action(AppAction::SelectInpaintTool(InpaintTool::Remove));

        assert!(app.inpaint.source_pick_active);
        assert_eq!(app.inpaint.hovered_stroke, Some(2));
    }
}
