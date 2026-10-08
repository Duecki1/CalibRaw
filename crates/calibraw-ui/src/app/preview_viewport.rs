//! Presents the preview canvas. The views in `ui::preview::viewport` lay out
//! and navigate the canvas and return `PreviewViewportAction`s; this module
//! applies them between the phases, so each phase reads the state the
//! previous one left.

use super::*;
use crate::ui::preview::{Preview, PreviewDetailImage, PreviewPlaceholder, PreviewViewportInput};

pub(crate) enum PreviewViewportAction {
    SetSourceAxesSwapped(bool),
    SetTouchNavigation(bool),
    SetNavigation {
        zoom: f32,
        center: [f32; 2],
    },
    /// The canvas area showing the image, in physical pixels.
    SetViewportPixels([u32; 2]),
    /// The source region in view; a larger one may need a sharper preview.
    SetVisibleUv(PreviewUvRect),
    NoteMotion,
    EndWhiteBalancePickerDrag,
    CancelMaskGesture,
    CancelCropDrag,
    CancelInpaintStroke,
}

impl CalibRawApp {
    /// Show the preview and run the tool that owns the canvas. `unobscured`
    /// is the part of the canvas not covered by overlay panels.
    pub(crate) fn show_preview(
        &mut self,
        ui: &mut egui::Ui,
        frame: &eframe::Frame,
        unobscured: Option<egui::Rect>,
    ) {
        let canvas = Preview::begin_canvas(ui, self.preview_backdrop_color(), unobscured);
        if self.preview_base_pipeline().is_none() && self.preview_is_preparing() {
            self.refresh_develop_loading_thumbnail(ui.ctx());
        }
        let [clipping_base, clipping_detail] = self.preview_clipping_textures(frame);
        let base = self.preview_base_pipeline().map(|presented| {
            (
                clipping_base.unwrap_or(presented.texture()),
                presented.gpu().width,
                presented.gpu().height,
            )
        });
        let Some((texture_id, pipeline_width, pipeline_height)) = base else {
            if canvas.has_area() {
                self.set_preview_viewport_pixels(canvas.physical_pixels(ui.ctx()));
            }
            Preview::show_placeholder(ui, canvas, &PreviewPlaceholder::of(self), unobscured);
            return;
        };

        let input = PreviewViewportInput::of(self, (pipeline_width, pipeline_height));
        let Some((mut viewport, actions)) = Preview::begin_viewport(ui, canvas, &input, unobscured)
        else {
            return;
        };
        self.apply_preview_viewport_actions(actions);

        #[cfg(target_os = "android")]
        let original_hold_tracking = !viewport.point_color_canvas() && {
            let hold = Preview::handle_android_original_hold(
                ui,
                self,
                viewport.interaction_rect(),
                viewport.touch_navigation(),
            );
            if hold.original_started {
                // The press began as a brush stroke, mask/crop drag or retouch;
                // holding still turns it into the comparison instead.
                if let Some(action) = Preview::tool_gesture_cancellation(self.ui.sidebar_tab) {
                    self.apply_preview_viewport_action(action);
                }
            }
            hold.tracking
        };
        #[cfg(not(target_os = "android"))]
        let original_hold_tracking = false;

        let actions = Preview::navigate_viewport(
            ui,
            &mut viewport,
            self.preview.visible_uv,
            original_hold_tracking,
        );
        self.apply_preview_viewport_actions(actions);

        Preview::paint_preview_image(
            ui,
            &viewport,
            texture_id,
            PreviewDetailImage::of(self, clipping_detail).as_ref(),
        );
        Preview::show_preview_tools(ui, self, frame, &viewport);
    }

    fn apply_preview_viewport_actions(&mut self, actions: Vec<PreviewViewportAction>) {
        for action in actions {
            self.apply_preview_viewport_action(action);
        }
    }

    fn apply_preview_viewport_action(&mut self, action: PreviewViewportAction) {
        match action {
            PreviewViewportAction::SetSourceAxesSwapped(swapped) => {
                self.preview.source_axes_swapped = swapped;
            }
            PreviewViewportAction::SetTouchNavigation(active) => {
                self.preview.touch_navigation_active = active;
            }
            PreviewViewportAction::SetNavigation { zoom, center } => {
                self.preview.zoom = zoom;
                self.preview.center = center;
            }
            PreviewViewportAction::SetViewportPixels(pixels) => {
                self.set_preview_viewport_pixels(pixels);
            }
            PreviewViewportAction::SetVisibleUv(visible_uv) => {
                self.preview.visible_uv = visible_uv;
                self.preview_source_region_changed();
            }
            PreviewViewportAction::NoteMotion => self.note_preview_motion(),
            PreviewViewportAction::EndWhiteBalancePickerDrag => {
                self.develop_ui.white_balance_picker_drag = None;
            }
            PreviewViewportAction::CancelMaskGesture => self.cancel_mask_touch_gesture(),
            PreviewViewportAction::CancelCropDrag => {
                self.develop_ui.crop_drag = None;
                self.develop_ui.straighten_drag = None;
            }
            PreviewViewportAction::CancelInpaintStroke => {
                self.inpaint.active_points.clear();
                self.inpaint.last_brush_uv = None;
            }
        }
    }
}
