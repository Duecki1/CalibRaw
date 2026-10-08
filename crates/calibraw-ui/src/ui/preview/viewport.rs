//! The preview canvas: placeholder states, navigation (zoom, pan, pinch and
//! fit) and the developed image. These views read narrow inputs and return
//! `PreviewViewportAction`s; `CalibRawApp::show_preview` applies them between
//! the phases, so the app state each phase reads matches the frame order.

use super::*;
use crate::app::{PreviewUvRect, PreviewViewportAction};

/// The preview area after the backdrop is painted.
#[derive(Clone, Copy)]
pub(crate) struct PreviewCanvas {
    available: egui::Vec2,
    inset: f32,
    /// The area left for the image inside the inset.
    preview_size: egui::Vec2,
    backdrop: Color32,
}

impl PreviewCanvas {
    pub(crate) fn has_area(self) -> bool {
        self.preview_size.x > 0.0 && self.preview_size.y > 0.0
    }

    fn is_empty(self) -> bool {
        self.preview_size.x <= 0.0 || self.preview_size.y <= 0.0
    }

    /// The image area in physical pixels.
    pub(crate) fn physical_pixels(self, ctx: &egui::Context) -> [u32; 2] {
        let pixels_per_point = physical_pixels_per_point(ctx);
        [
            (self.preview_size.x * pixels_per_point).round().max(1.0) as u32,
            (self.preview_size.y * pixels_per_point).round().max(1.0) as u32,
        ]
    }
}

/// What the placeholder shows while no developed image exists.
pub(crate) struct PreviewPlaceholder<'a> {
    preparing: bool,
    loading_thumbnail: Option<(&'a egui::TextureHandle, [u32; 2])>,
}

impl<'a> PreviewPlaceholder<'a> {
    pub(crate) fn of(app: &'a CalibRawApp) -> Self {
        let thumbnail = &app.develop_ui.loading_thumbnail;
        Self {
            preparing: app.preview_is_preparing(),
            loading_thumbnail: thumbnail.texture.as_ref().zip(thumbnail.texture_size),
        }
    }
}

/// The app state navigation starts from.
pub(crate) struct PreviewViewportInput {
    /// Size of the developed preview texture.
    pipeline_size: (u32, u32),
    /// Size of the loaded source, when it is known.
    source_size: Option<(u32, u32)>,
    lens_geometry: Option<Arc<LensGeometryMap>>,
    geometry: GeometryTransform,
    sidebar_tab: SidebarTab,
    original_visible: bool,
    white_balance_picker_active: bool,
    point_color_picker_active: bool,
    zoom: f32,
    center: [f32; 2],
    touch_navigation_active: bool,
    space_pan_active: bool,
}

impl PreviewViewportInput {
    pub(crate) fn of(app: &CalibRawApp, pipeline_size: (u32, u32)) -> Self {
        Self {
            pipeline_size,
            source_size: app
                .develop
                .loaded_raw
                .as_ref()
                .map(|raw| (raw.width, raw.height)),
            lens_geometry: loaded_lens_geometry(app).cloned(),
            geometry: app.develop.geometry,
            sidebar_tab: app.ui.sidebar_tab,
            original_visible: app.preview.original_visible(),
            white_balance_picker_active: app.develop_ui.white_balance_picker_active,
            point_color_picker_active: match app.ui.sidebar_tab {
                SidebarTab::Masks => app.develop_ui.mask_point_color.picker_active,
                _ => app.develop_ui.point_color.picker_active,
            },
            zoom: app.preview.zoom,
            center: app.preview.center,
            touch_navigation_active: app.preview.touch_navigation_active,
            space_pan_active: app.preview.space_pan_active,
        }
    }
}

/// The canvas geometry and gesture state of one frame, carried from
/// navigation to painting and the tools.
pub(crate) struct PreviewViewport {
    canvas_rect: Rect,
    /// The unobscured canvas; navigation also works in its margins.
    outer_rect: Rect,
    image_rect: Rect,
    /// The image size at zoom 1.
    base_size: egui::Vec2,
    zoom: f32,
    center: [f32; 2],
    source_size: (u32, u32),
    /// Width of the displayed image (after crop or quarter turns) in source pixels.
    geometry_width: u32,
    geometry: GeometryTransform,
    lens_geometry: Option<Arc<LensGeometryMap>>,
    sidebar_tab: SidebarTab,
    crop_preview: bool,
    final_geometry_preview: bool,
    white_balance_canvas: bool,
    point_color_canvas: bool,
    brush_canvas: bool,
    touch_navigation: bool,
    /// Space-drag navigation owns the primary button; see `space_pan`.
    space_pan: bool,
    fit_gesture: bool,
    moved: bool,
    response: egui::Response,
}

impl PreviewViewport {
    #[cfg(target_os = "android")]
    pub(crate) fn point_color_canvas(&self) -> bool {
        self.point_color_canvas
    }

    #[cfg(target_os = "android")]
    pub(crate) fn interaction_rect(&self) -> Rect {
        self.outer_rect
    }

    #[cfg(target_os = "android")]
    pub(crate) fn touch_navigation(&self) -> bool {
        self.touch_navigation
    }

    fn layout(&self) -> PreviewLayout {
        PreviewLayout {
            image_rect: self.image_rect,
            visible_rect: self.outer_rect.intersect(self.image_rect),
            viewport_rect: self.outer_rect,
            source_width: self.source_size.0,
            source_height: self.source_size.1,
        }
    }

    fn navigation(&self) -> PreviewViewportAction {
        PreviewViewportAction::SetNavigation {
            zoom: self.zoom,
            center: self.center,
        }
    }

    /// The zoom at which one displayed photo pixel covers one physical screen
    /// pixel. Unclamped; callers clamp to the zoom range.
    fn native_zoom(&self, ctx: &egui::Context) -> f32 {
        self.geometry_width as f32 / (self.base_size.x * physical_pixels_per_point(ctx)).max(1.0)
    }

    fn update_image_rect(&mut self) {
        self.image_rect =
            zoomed_image_rect(self.outer_rect, self.base_size, self.zoom, self.center);
    }

    /// Zoom by `zoom_delta` about `previous`, moving that point to `current`.
    fn transform_about(&mut self, previous: Pos2, current: Pos2, zoom_delta: f32) -> bool {
        transform_preview_about_screen_points(
            self.outer_rect,
            self.image_rect,
            self.base_size,
            &mut self.zoom,
            &mut self.center,
            previous,
            current,
            zoom_delta,
        )
    }
}

/// The detail render covering part of the image at higher resolution.
pub(crate) struct PreviewDetailImage {
    texture_id: egui::TextureId,
    texture_uv: Rect,
    source_uv: PreviewUvRect,
}

impl PreviewDetailImage {
    /// The current detail render, or `None` while it belongs to an older revision.
    pub(crate) fn of(app: &CalibRawApp, clipping_texture: Option<egui::TextureId>) -> Option<Self> {
        let detail = app
            .preview
            .detail
            .as_ref()
            .filter(|detail| detail.revision == app.preview.revision)?;
        Some(Self {
            texture_id: clipping_texture.unwrap_or(detail.pipeline.texture()),
            texture_uv: Rect::from_min_max(
                Pos2::new(detail.texture_uv_rect.min[0], detail.texture_uv_rect.min[1]),
                Pos2::new(detail.texture_uv_rect.max[0], detail.texture_uv_rect.max[1]),
            ),
            source_uv: detail.uv_rect,
        })
    }
}

impl Preview {
    /// Fill the preview area with the backdrop and measure the canvas.
    pub(crate) fn begin_canvas(
        ui: &mut Ui,
        backdrop: Color32,
        unobscured: Option<Rect>,
    ) -> PreviewCanvas {
        let available = ui.available_size();
        let inset = if unobscured.is_some() || cfg!(target_os = "android") {
            0.0
        } else {
            10.0
        };
        let preview_size = (available - egui::Vec2::splat(inset * 2.0)).max(egui::Vec2::ZERO);
        ui.painter()
            .rect_filled(ui.available_rect_before_wrap(), 0.0, backdrop);
        PreviewCanvas {
            available,
            inset,
            preview_size,
            backdrop,
        }
    }

    pub(crate) fn show_placeholder(
        ui: &mut Ui,
        canvas: PreviewCanvas,
        placeholder: &PreviewPlaceholder<'_>,
        unobscured: Option<Rect>,
    ) {
        if placeholder.preparing {
            if show_loading_thumbnail(
                ui,
                placeholder.loading_thumbnail,
                canvas.available,
                unobscured,
            ) {
                return;
            }
            show_centered_preview_message(
                ui,
                canvas.available,
                canvas.backdrop,
                "Preparing preview…",
                None,
                true,
            );
        } else {
            show_centered_preview_message(
                ui,
                canvas.available,
                canvas.backdrop,
                "No image open",
                Some("Open a photo from the Library to start developing."),
                false,
            );
        }
    }

    /// Lay out the canvas, decide which tool owns it and follow a pinch.
    /// Returns `None` when there is no area to show the image in.
    pub(crate) fn begin_viewport(
        ui: &mut Ui,
        canvas: PreviewCanvas,
        input: &PreviewViewportInput,
        unobscured: Option<Rect>,
    ) -> Option<(PreviewViewport, Vec<PreviewViewportAction>)> {
        if canvas.is_empty() || input.pipeline_size.1 == 0 {
            return None;
        }
        let mut actions = Vec::new();
        let (workspace_rect, _) = ui.allocate_exact_size(canvas.available, Sense::hover());
        let canvas_rect = workspace_rect.shrink(canvas.inset);
        let outer_rect = unobscured.unwrap_or(canvas_rect).intersect(canvas_rect);
        let source_size = input.source_size.unwrap_or(input.pipeline_size);
        let crop_preview = input.sidebar_tab == SidebarTab::Crop && !input.original_visible;
        let final_geometry_preview =
            !crop_preview && (!input.geometry.is_identity() || input.lens_geometry.is_some());
        let (geometry_width, geometry_height) = if final_geometry_preview {
            input
                .geometry
                .crop_pixel_dimensions(source_size.0, source_size.1)
        } else if crop_preview && input.geometry.quarter_turns % 2 == 1 {
            (source_size.1, source_size.0)
        } else {
            source_size
        };
        let image_aspect = geometry_width as f32 / geometry_height.max(1) as f32;
        let base_size = if unobscured.is_some() && canvas_rect.height() > canvas_rect.width() {
            // Portrait has a width-fitted image, independent of the tool sheet.
            egui::vec2(
                canvas_rect.width(),
                canvas_rect.width() / image_aspect.max(f32::EPSILON),
            )
        } else {
            fitted_image_size(canvas_rect.size(), image_aspect)
        };
        actions.push(PreviewViewportAction::SetSourceAxesSwapped(
            input.geometry.quarter_turns % 2 == 1,
        ));
        let zoom = input.zoom.clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM);
        let mut center = input.center;
        clamp_preview_center(&mut center, outer_rect.size(), base_size * zoom);

        let white_balance_canvas =
            white_balance_picker_owns_canvas(input.sidebar_tab, input.white_balance_picker_active);
        let point_color_canvas =
            point_color_picker_owns_canvas(input.sidebar_tab, input.point_color_picker_active);
        if !white_balance_canvas {
            actions.push(PreviewViewportAction::EndWhiteBalancePickerDrag);
        }
        let brush_canvas = matches!(
            input.sidebar_tab,
            SidebarTab::Masks | SidebarTab::Inpainting
        ) || white_balance_canvas;
        let response = Self::canvas_response(
            ui,
            outer_rect,
            input.sidebar_tab,
            white_balance_canvas,
            point_color_canvas,
        );

        let (multi_touch, any_touches) =
            ui.input(|input| (input.multi_touch(), input.any_touches()));
        let multi_touch = multi_touch.filter(|touch| {
            outer_rect.contains(touch.start_pos)
                && ui.ctx().layer_id_at(touch.start_pos) == Some(ui.layer_id())
        });
        #[cfg(target_os = "android")]
        if any_touches {
            ui.ctx().request_repaint();
        }
        let touch_navigation = if multi_touch.is_some() {
            true
        } else if !any_touches {
            false
        } else {
            input.touch_navigation_active
        };
        actions.push(PreviewViewportAction::SetTouchNavigation(touch_navigation));
        let space_pan = Self::space_pan(ui, &response, input.space_pan_active);
        actions.push(PreviewViewportAction::SetSpacePan(space_pan));

        let mut viewport = PreviewViewport {
            canvas_rect,
            outer_rect,
            image_rect: zoomed_image_rect(outer_rect, base_size, zoom, center),
            base_size,
            zoom,
            center,
            source_size,
            geometry_width,
            geometry: input.geometry,
            lens_geometry: input.lens_geometry.clone(),
            sidebar_tab: input.sidebar_tab,
            crop_preview,
            final_geometry_preview,
            white_balance_canvas,
            point_color_canvas,
            brush_canvas,
            touch_navigation,
            space_pan,
            fit_gesture: false,
            moved: false,
            response,
        };
        if let Some(multi_touch) = multi_touch {
            let previous_touch_center = multi_touch.center_pos - multi_touch.translation_delta;
            viewport.moved |= viewport.transform_about(
                previous_touch_center,
                multi_touch.center_pos,
                multi_touch.zoom_delta,
            );
            viewport.update_image_rect();
        }
        actions.push(PreviewViewportAction::SetNativeZoom(
            viewport.native_zoom(ui.ctx()),
        ));
        // Cancelling a tool gesture reprocesses at the new zoom.
        actions.push(viewport.navigation());
        if multi_touch.is_some() {
            actions.extend(Self::second_finger_cancellation(
                input.sidebar_tab,
                white_balance_canvas,
            ));
        }
        Some((viewport, actions))
    }

    /// Holding Space over the preview turns the primary button into a pan
    /// in every tab, as on laptops without a middle button. It arms only
    /// while the button is up, so pressing Space mid-stroke does not cut a
    /// stroke short, and a pan lasts until the button is released.
    pub(super) fn space_pan(ui: &Ui, response: &egui::Response, active: bool) -> bool {
        let text_input = ui.ctx().egui_wants_keyboard_input();
        let (space_down, primary_down, primary_released) = ui.input(|input| {
            (
                input.key_down(egui::Key::Space),
                input.pointer.primary_down(),
                input.pointer.primary_released(),
            )
        });
        let space_held = space_down && !text_input;
        let active = if active {
            // Through the release frame too, so no tool sees the release.
            space_held || primary_down || primary_released
        } else {
            // Never on a release frame: that release ends a tool stroke.
            space_held && !primary_down && !primary_released && response.hovered()
        };
        if active {
            // Keep Space from also activating a focused button.
            ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Space));
            ui.ctx().set_cursor_icon(if primary_down {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            });
        }
        active
    }

    fn canvas_response(
        ui: &mut Ui,
        interaction_rect: Rect,
        sidebar_tab: SidebarTab,
        white_balance_canvas: bool,
        point_color_canvas: bool,
    ) -> egui::Response {
        let interaction_id = match sidebar_tab {
            SidebarTab::Masks if point_color_canvas => {
                ui.id().with("develop-preview-mask-point-color-interaction")
            }
            SidebarTab::Masks => ui.id().with("develop-preview-mask-interaction"),
            SidebarTab::Inpainting => ui.id().with("develop-preview-inpaint-interaction"),
            SidebarTab::Adjustments if white_balance_canvas => {
                ui.id().with("develop-preview-white-balance-interaction")
            }
            SidebarTab::Adjustments if point_color_canvas => {
                ui.id().with("develop-preview-point-color-interaction")
            }
            _ => ui.id().with("develop-preview-interaction"),
        };
        let interaction_sense = if white_balance_canvas || point_color_canvas {
            Sense::drag()
        } else {
            Sense::click_and_drag()
        };
        ui.interact(interaction_rect, interaction_id, interaction_sense)
    }

    /// A second finger turns a tool gesture into navigation.
    fn second_finger_cancellation(
        sidebar_tab: SidebarTab,
        white_balance_canvas: bool,
    ) -> Option<PreviewViewportAction> {
        Self::tool_gesture_cancellation(sidebar_tab).or_else(|| {
            white_balance_canvas.then_some(PreviewViewportAction::EndWhiteBalancePickerDrag)
        })
    }

    /// Reverts the stroke or drag a press started with the tab's canvas tool.
    pub(crate) fn tool_gesture_cancellation(
        sidebar_tab: SidebarTab,
    ) -> Option<PreviewViewportAction> {
        match sidebar_tab {
            SidebarTab::Masks => Some(PreviewViewportAction::CancelMaskGesture),
            SidebarTab::Crop => Some(PreviewViewportAction::CancelCropDrag),
            SidebarTab::Inpainting => Some(PreviewViewportAction::CancelInpaintStroke),
            _ => None,
        }
    }

    /// Scroll and pinch zoom, pan, and the double-click fit; then publish the
    /// visible region. `visible_uv` is the region requested last frame.
    pub(crate) fn navigate_viewport(
        ui: &Ui,
        viewport: &mut PreviewViewport,
        visible_uv: PreviewUvRect,
        original_hold_tracking: bool,
    ) -> Vec<PreviewViewportAction> {
        let mut actions = Vec::new();
        if !viewport.touch_navigation && viewport.response.hovered() {
            Self::zoom_at_pointer(ui, viewport);
        }
        if !viewport.touch_navigation {
            Self::zoom_with_keyboard(ui, viewport);
        }
        Self::pan(ui, viewport, original_hold_tracking);
        viewport.fit_gesture = !viewport.white_balance_canvas
            && !viewport.point_color_canvas
            && !viewport.touch_navigation
            && viewport.response.double_clicked();
        if viewport.fit_gesture {
            // Cancelling reprocesses at the zoom before the fit.
            actions.push(viewport.navigation());
            actions.extend([
                PreviewViewportAction::CancelMaskGesture,
                PreviewViewportAction::CancelInpaintStroke,
                PreviewViewportAction::CancelCropDrag,
            ]);
            Self::toggle_fit(ui, viewport);
        }
        actions.push(viewport.navigation());

        viewport.update_image_rect();
        // The fitted image continues behind tools; spend the detail budget on
        // the exposed image, not the pixels hidden by the overlay surfaces.
        let visible_screen = viewport.outer_rect.intersect(viewport.image_rect);
        let pixels_per_point = physical_pixels_per_point(ui.ctx());
        actions.push(PreviewViewportAction::SetViewportPixels([
            (visible_screen.width() * pixels_per_point).round().max(1.0) as u32,
            (visible_screen.height() * pixels_per_point)
                .round()
                .max(1.0) as u32,
        ]));
        let new_visible_uv = Self::visible_source_uv(viewport, visible_screen);
        if preview_uv_changed(visible_uv, new_visible_uv) {
            actions.push(PreviewViewportAction::SetVisibleUv(new_visible_uv));
            viewport.moved = true;
        }
        if viewport.moved {
            actions.push(PreviewViewportAction::NoteMotion);
        }
        actions
    }

    fn zoom_at_pointer(ui: &Ui, viewport: &mut PreviewViewport) {
        let (scroll_y, pinch_zoom) =
            ui.input(|input| (input.smooth_scroll_delta.y, input.zoom_delta()));
        if scroll_y.abs() > 0.01 || (pinch_zoom - 1.0).abs() > f32::EPSILON {
            let pointer = ui
                .input(|input| input.pointer.hover_pos())
                .unwrap_or(viewport.outer_rect.center());
            viewport.moved |=
                viewport.transform_about(pointer, pointer, (scroll_y * 0.0018).exp() * pinch_zoom);
        }
    }

    /// Ctrl/Cmd +/- step the zoom, Ctrl/Cmd 1 shows 100% (1:1) and Ctrl/Cmd 0
    /// fits. Steps and 100% keep the pointer's pixel in place when the pointer
    /// is over the preview, otherwise the preview's centre.
    fn zoom_with_keyboard(ui: &Ui, viewport: &mut PreviewViewport) {
        let Some(zoom) = ui.input_mut(keyboard_zoom) else {
            return;
        };
        let anchor = ui
            .input(|input| input.pointer.hover_pos())
            .filter(|pointer| viewport.outer_rect.contains(*pointer))
            .unwrap_or(viewport.outer_rect.center());
        let factor = match zoom {
            KeyboardZoom::Step(factor) => factor,
            KeyboardZoom::Native => {
                viewport
                    .native_zoom(ui.ctx())
                    .clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM)
                    / viewport.zoom
            }
            KeyboardZoom::Fit => {
                viewport.zoom = 1.0;
                viewport.center = [0.5, 0.5];
                viewport.moved = true;
                return;
            }
        };
        viewport.moved |= viewport.transform_about(anchor, anchor, factor);
    }

    fn pan(ui: &Ui, viewport: &mut PreviewViewport, original_hold_tracking: bool) {
        let response = &viewport.response;
        let pan_with_primary = !viewport.touch_navigation
            && !original_hold_tracking
            && !viewport.brush_canvas
            && !viewport.point_color_canvas
            && viewport.sidebar_tab != SidebarTab::Crop
            && response.dragged_by(egui::PointerButton::Primary);
        let pan_with_middle =
            !viewport.touch_navigation && response.dragged_by(egui::PointerButton::Middle);
        let pan_with_space =
            viewport.space_pan && response.dragged_by(egui::PointerButton::Primary);
        if pan_with_primary || pan_with_middle || pan_with_space {
            let delta = ui.input(|input| input.pointer.delta());
            let image_size = viewport.base_size * viewport.zoom;
            viewport.center[0] -= delta.x / image_size.x.max(1.0);
            viewport.center[1] -= delta.y / image_size.y.max(1.0);
            clamp_preview_center(&mut viewport.center, viewport.outer_rect.size(), image_size);
            viewport.moved |= delta.length_sq() > 0.0;
        }
    }

    /// Return to the fitted view, or from it zoom to native pixels at the pointer.
    fn toggle_fit(ui: &Ui, viewport: &mut PreviewViewport) {
        if (viewport.zoom - 1.0).abs() > 0.0005 {
            viewport.zoom = 1.0;
            viewport.center = [0.5, 0.5];
        } else {
            let native_zoom = viewport.native_zoom(ui.ctx()).clamp(1.0, MAX_PREVIEW_ZOOM);
            let pointer = viewport
                .response
                .interact_pointer_pos()
                .unwrap_or(viewport.outer_rect.center());
            viewport.transform_about(pointer, pointer, native_zoom);
        }
        viewport.moved = true;
    }

    fn visible_source_uv(viewport: &PreviewViewport, visible_screen: Rect) -> PreviewUvRect {
        let image_rect = viewport.image_rect;
        if viewport.crop_preview {
            crop_workspace_visible_source_uv(
                image_rect,
                visible_screen,
                viewport.geometry,
                viewport.lens_geometry.as_deref(),
                viewport.source_size.0,
                viewport.source_size.1,
            )
        } else if viewport.final_geometry_preview {
            viewport
                .layout()
                .projection(viewport.geometry, viewport.lens_geometry.as_deref())
                .visible_source_uv(visible_screen)
        } else {
            PreviewUvRect {
                min: [
                    ((visible_screen.left() - image_rect.left()) / image_rect.width().max(1.0))
                        .clamp(0.0, 1.0),
                    ((visible_screen.top() - image_rect.top()) / image_rect.height().max(1.0))
                        .clamp(0.0, 1.0),
                ],
                max: [
                    ((visible_screen.right() - image_rect.left()) / image_rect.width().max(1.0))
                        .clamp(0.0, 1.0),
                    ((visible_screen.bottom() - image_rect.top()) / image_rect.height().max(1.0))
                        .clamp(0.0, 1.0),
                ],
            }
        }
    }

    /// Paint the developed image and, over it, the current detail render.
    pub(crate) fn paint_preview_image(
        ui: &Ui,
        viewport: &PreviewViewport,
        texture_id: egui::TextureId,
        detail: Option<&PreviewDetailImage>,
    ) {
        let projection = viewport
            .layout()
            .projection(viewport.geometry, viewport.lens_geometry.as_deref());
        let painter = ui.painter_at(viewport.canvas_rect);
        let full_texture = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        if viewport.crop_preview {
            paint_crop_workspace_texture(
                ui,
                texture_id,
                projection,
                full_texture,
                [0.0, 0.0, 1.0, 1.0],
            );
        } else if viewport.final_geometry_preview {
            paint_final_geometry_texture(
                ui,
                texture_id,
                projection,
                full_texture,
                [0.0, 0.0, 1.0, 1.0],
            );
        } else {
            painter.image(
                texture_id,
                viewport.image_rect,
                full_texture,
                Color32::WHITE,
            );
        }

        let Some(detail) = detail else {
            return;
        };
        let source_uv = detail.source_uv;
        let detail_source_uv = [
            source_uv.min[0],
            source_uv.min[1],
            source_uv.max[0],
            source_uv.max[1],
        ];
        if viewport.crop_preview {
            paint_crop_workspace_texture(
                ui,
                detail.texture_id,
                projection,
                detail.texture_uv,
                detail_source_uv,
            );
        } else if viewport.final_geometry_preview {
            paint_final_geometry_texture(
                ui,
                detail.texture_id,
                projection,
                detail.texture_uv,
                detail_source_uv,
            );
        } else {
            let detail_rect = Rect::from_min_max(
                normalized_to_screen(viewport.image_rect, source_uv.min),
                normalized_to_screen(viewport.image_rect, source_uv.max),
            );
            painter.image(
                detail.texture_id,
                detail_rect,
                detail.texture_uv,
                Color32::WHITE,
            );
        }
    }

    /// Run the crop tool, label the canvas, then run and paint the other
    /// tools that own the canvas. The tool handlers still take the app.
    pub(crate) fn show_preview_tools(
        ui: &mut Ui,
        app: &mut CalibRawApp,
        frame: &eframe::Frame,
        viewport: &PreviewViewport,
    ) {
        let layout = viewport.layout();
        let gesture_free = !viewport.touch_navigation
            && !viewport.space_pan
            && !viewport.fit_gesture
            && !app.preview.original_hold_owns_touch();
        if viewport.crop_preview {
            if gesture_free {
                Self::handle_crop_interaction(ui, app, layout);
            }
            Self::paint_crop_overlay(ui, app, layout);
        }

        paint_preview_labels(
            &ui.painter_at(viewport.canvas_rect),
            viewport.outer_rect,
            app.preview.original_visible(),
            app.current_rendered_format(),
        );
        if app.preview.original_visible() {
            return;
        }

        let response = &viewport.response;
        if app.ui.sidebar_tab == SidebarTab::Inpainting && gesture_free {
            Self::handle_inpaint_interaction(ui, app, frame, layout, response);
        }
        Self::paint_inpaint_overlay(ui, app, layout);

        if viewport.white_balance_canvas {
            if !viewport.touch_navigation && !viewport.space_pan {
                Self::handle_white_balance_picker(ui, app, layout, response);
            }
            Self::paint_white_balance_picker(ui, app, layout);
        }

        if viewport.point_color_canvas {
            if !viewport.touch_navigation && !viewport.space_pan {
                Self::handle_point_color_picker(ui, app, frame, layout, response);
            }
            Self::paint_point_color_picker(ui, app, layout.visible_rect, response);
        }

        if app.ui.sidebar_tab == SidebarTab::Masks {
            if gesture_free && !viewport.point_color_canvas {
                let actions =
                    Self::mask_tool_actions(ui, &tools::MaskToolInput::of(app), layout, response);
                app.apply_mask_tool_actions(ui.ctx(), actions);
            }
            Self::paint_mask_overlay(ui, app, layout);
            Self::paint_tool_hint(ui, app, layout.visible_rect);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum KeyboardZoom {
    /// Multiply the zoom by this factor.
    Step(f32),
    Fit,
    /// 100%: one photo pixel per physical screen pixel.
    Native,
}

/// Consumes the preview zoom shortcuts: Ctrl/Cmd +/- (egui's interface-zoom
/// shortcuts, which CalibRaw disables), Ctrl/Cmd 0 and Ctrl/Cmd 1.
pub(super) fn keyboard_zoom(input: &mut egui::InputState) -> Option<KeyboardZoom> {
    use egui::gui_zoom::kb_shortcuts::{ZOOM_IN, ZOOM_IN_SECONDARY, ZOOM_OUT};
    const STEP: f32 = 1.25;

    if input.consume_shortcut(&ZOOM_FIT_SHORTCUT) {
        return Some(KeyboardZoom::Fit);
    }
    if input.consume_shortcut(&ZOOM_NATIVE_SHORTCUT) {
        return Some(KeyboardZoom::Native);
    }
    let zoom_in = input.consume_shortcut(&ZOOM_IN) | input.consume_shortcut(&ZOOM_IN_SECONDARY);
    match (zoom_in, input.consume_shortcut(&ZOOM_OUT)) {
        (true, false) => Some(KeyboardZoom::Step(STEP)),
        (false, true) => Some(KeyboardZoom::Step(STEP.recip())),
        _ => None,
    }
}

/// "ORIGINAL" while the unedited image shows, and the rendered-format pill.
fn paint_preview_labels(
    painter: &egui::Painter,
    outer_rect: Rect,
    original_visible: bool,
    rendered_format: Option<crate::pipeline::RenderedImageFormat>,
) {
    if original_visible {
        painter.text(
            outer_rect.right_top() + egui::vec2(-12.0, 12.0),
            egui::Align2::RIGHT_TOP,
            "ORIGINAL",
            egui::FontId::proportional(12.0),
            Color32::WHITE,
        );
    }
    if let Some(format) = rendered_format {
        paint_rendered_source_pill(painter, outer_rect, format);
    }
}
