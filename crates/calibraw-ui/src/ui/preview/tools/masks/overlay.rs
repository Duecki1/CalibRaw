//! Painting mask overlays, coverage textures and tool hints over the preview.

use super::*;

impl Preview {
    pub(in crate::ui::preview) fn paint_mask_overlay(
        ui: &Ui,
        app: &mut CalibRawApp,
        layout: PreviewLayout,
    ) {
        let PreviewLayout {
            image_rect,
            visible_rect,
            viewport_rect,
            ..
        } = layout;
        let lens_geometry = loaded_lens_geometry(app).cloned();
        let projection = layout.projection(app.develop.geometry, lens_geometry.as_deref());
        // Radial and linear shapes are stored in lens-corrected coordinates.
        let corrected_projection = projection.without_lens();
        let Some(mask_index) = app.masks.stack.selected_mask else {
            return;
        };
        let Some(mask) = app.masks.stack.masks.get(mask_index) else {
            return;
        };
        let selected_component = app.masks.stack.selected_component;
        let neutral = !mask.has_active_edit();
        let accent = selected_component
            .map(mask_component_color)
            .unwrap_or(crate::ui::theme::MASK_ADD);
        let subtract = crate::ui::theme::MASK_SUBTRACT;
        let painter = ui.painter_at(viewport_rect);

        let force_overlay =
            crate::app::preview_visibility::PreviewVisibility::mask_overlay_forced(ui.ctx());
        let steady_target: Option<Option<usize>> = (neutral || force_overlay).then_some(None);
        let hidden_target = if force_overlay { steady_target } else { None };
        let mut coverage_target = steady_target;
        if let Some((started, blink)) = app.masks.overlay_blink {
            let elapsed = started.elapsed().as_secs_f32();
            coverage_target = match blink {
                MaskOverlayBlink::GroupTwice if elapsed < 0.18 => Some(None),
                MaskOverlayBlink::GroupTwice if elapsed < 0.32 => hidden_target,
                MaskOverlayBlink::GroupTwice if elapsed < 0.50 => Some(None),
                MaskOverlayBlink::GroupTwice if elapsed < 0.64 => hidden_target,
                MaskOverlayBlink::ComponentThenGroup if elapsed < 0.22 => {
                    selected_component.map(Some)
                }
                MaskOverlayBlink::ComponentThenGroup if elapsed < 0.35 => hidden_target,
                MaskOverlayBlink::ComponentThenGroup if elapsed < 0.57 => Some(None),
                MaskOverlayBlink::ComponentThenGroup if elapsed < 0.70 => hidden_target,
                _ => {
                    app.masks.overlay_blink = None;
                    steady_target
                }
            };
            if app.masks.overlay_blink.is_some() {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(25));
            }
        }
        let pointer_editing = ui.input(|input| input.pointer.primary_down())
            && ui
                .ctx()
                .pointer_interact_pos()
                .is_some_and(|position| visible_rect.contains(position));
        if pointer_editing {
            let editing_live_mask = selected_component.is_some_and(|index| {
                app.masks.stack.masks[mask_index]
                    .components
                    .get(index)
                    .is_some_and(|component| {
                        component.kind == MaskKind::Object
                            || (app.masks.subject_refinement_active
                                && matches!(
                                    component.kind,
                                    MaskKind::Subject | MaskKind::Background
                                ))
                            || (neutral
                                && !matches!(
                                    component.kind,
                                    MaskKind::Subject | MaskKind::Background
                                ))
                    })
            });
            coverage_target = if editing_live_mask {
                selected_component.map(Some)
            } else {
                hidden_target
            };
        }
        if app.develop_ui.mask_point_color_tab
            && (app.develop_ui.mask_point_color.visualize_range
                || app.develop_ui.mask_point_color.picker_active)
        {
            coverage_target = None;
        }
        if mask.enabled || force_overlay {
            if let Some(component) = coverage_target {
                Self::paint_coverage_texture(ui, app, layout, mask_index, component);
            }
        }

        if let Some(component) = selected_component.and_then(|index| {
            app.masks
                .stack
                .masks
                .get(mask_index)
                .and_then(|mask| mask.components.get(index))
        }) {
            if !component.enabled {
                return;
            }
            let color = accent;
            match &component.geometry {
                MaskGeometry::Brush { .. } => {}
                MaskGeometry::Radial {
                    center,
                    radius,
                    rotation,
                    feather,
                    initialized: true,
                } => {
                    let outer =
                        corrected_projection.radial_outline(*center, *radius, *rotation, 72);
                    painter.add(egui::Shape::line(outer, Stroke::new(2.0, color)));
                    let inner_scale = 1.0 - feather.clamp(0.0, 1.0) * 0.98;
                    let inner = corrected_projection.radial_outline(
                        *center,
                        [radius[0] * inner_scale, radius[1] * inner_scale],
                        *rotation,
                        72,
                    );
                    painter.add(egui::Shape::line(
                        inner,
                        Stroke::new(1.0, color.gamma_multiply(0.65)),
                    ));
                    let center_screen = corrected_projection.to_screen(*center);
                    let axis_handles =
                        corrected_projection.radial_handles(*center, *radius, *rotation);
                    let rotation_handle =
                        corrected_projection.radial_rotation_handle(*center, *radius, *rotation);
                    handles::paint_stem(&painter, axis_handles[0], rotation_handle, color);
                    handles::paint_point(&painter, center_screen, handles::POINT_RADIUS, color);
                    for handle in axis_handles {
                        handles::paint_point(
                            &painter,
                            handle,
                            handles::SECONDARY_POINT_RADIUS,
                            color,
                        );
                    }
                    handles::paint_ring(
                        &painter,
                        rotation_handle,
                        handles::ROTATION_RADIUS,
                        Stroke::new(2.0, color),
                    );
                }
                MaskGeometry::Linear {
                    start,
                    end,
                    feather,
                    initialized: true,
                } => {
                    let axis = corrected_projection.linear_axis(*start, *end, 48);
                    painter.add(Shape::line(
                        axis,
                        Stroke::new(1.0, color.gamma_multiply(0.65)),
                    ));
                    let a = corrected_projection.to_screen(*start);
                    let b = corrected_projection.to_screen(*end);
                    let (middle, rotation_handle) =
                        corrected_projection.linear_rotation_handle(*start, *end);

                    let width_factor = feather.clamp(0.02, 1.0);
                    let center_line = corrected_projection.linear_isoline(*start, *end, 0.5, 64);
                    painter.add(Shape::line(center_line, Stroke::new(2.0, color)));
                    for t in [0.5 - 0.5 * width_factor, 0.5 + 0.5 * width_factor] {
                        let boundary = corrected_projection.linear_isoline(*start, *end, t, 64);
                        painter.add(Shape::line(
                            boundary,
                            Stroke::new(1.0, color.gamma_multiply(0.65)),
                        ));
                    }
                    // Handles go over the guide lines they sit on.
                    handles::paint_stem(&painter, middle, rotation_handle, color);
                    handles::paint_point(&painter, a, handles::POINT_RADIUS, color);
                    handles::paint_point(&painter, b, handles::POINT_RADIUS, color);
                    handles::paint_ring(
                        &painter,
                        rotation_handle,
                        handles::ROTATION_RADIUS,
                        Stroke::new(2.0, color),
                    );
                }
                MaskGeometry::Path { points, .. } => {
                    if points.len() >= 3 {
                        let outline = crate::pipeline::path_outline_points(points, 16)
                            .into_iter()
                            .map(|uv| projection.to_screen(uv))
                            .collect::<Vec<_>>();
                        painter.add(Shape::line(outline, Stroke::new(2.0, color)));
                    } else if points.len() >= 2 {
                        let outline = points
                            .iter()
                            .map(|point| projection.to_screen(point.position))
                            .collect::<Vec<_>>();
                        painter.add(Shape::line(outline, Stroke::new(2.0, color)));
                    }
                    for point in points {
                        let anchor = projection.to_screen(point.position);
                        for handle in [point.incoming(), point.outgoing()] {
                            let dx = handle[0] - point.position[0];
                            let dy = handle[1] - point.position[1];
                            if dx * dx + dy * dy <= 1e-10 {
                                continue;
                            }
                            let handle_screen = projection.to_screen(handle);
                            handles::paint_stem(&painter, anchor, handle_screen, color);
                            handles::paint_ring(
                                &painter,
                                handle_screen,
                                handles::SECONDARY_POINT_RADIUS + 0.5,
                                Stroke::new(1.5, color),
                            );
                        }
                        handles::paint_point(&painter, anchor, handles::POINT_RADIUS, color);
                    }
                }
                _ => {}
            }
        }

        let refining_subject = app.masks.subject_refinement_active
            && app
                .masks
                .stack
                .selected_component()
                .is_some_and(|component| {
                    matches!(component.kind, MaskKind::Subject | MaskKind::Background)
                        && component.enabled
                });
        if refining_subject
            || app
                .masks
                .stack
                .selected_component()
                .is_some_and(|component| {
                    matches!(component.kind, MaskKind::Brush | MaskKind::Object)
                        && component.enabled
                })
        {
            if let Some(pointer) = ui
                .ctx()
                .pointer_hover_pos()
                .or_else(|| ui.ctx().pointer_interact_pos())
                .filter(|position| visible_rect.contains(*position))
            {
                let cursor_color = match app.masks.brush_mode {
                    BrushMode::Paint => Color32::WHITE,
                    BrushMode::Erase => subtract,
                };
                if refining_subject {
                    let source_uv = projection.to_source(pointer);
                    if let Some(uv) = editable_source_uv(source_uv) {
                        let brush_size = zoom_scaled_brush_size(
                            app.masks.stack.subject_refinement.size,
                            app.preview.zoom,
                            app.preferences.image_relative_brush_size,
                        );
                        let outline = projection.brush_outline(uv, brush_size, 64);
                        let cursor_painter = ui.painter_at(visible_rect.intersect(image_rect));
                        cursor_painter.add(Shape::line(outline, Stroke::new(1.5, cursor_color)));
                        let inner_size = brush_size
                            * (1.0 - app.masks.stack.subject_refinement.feather.clamp(0.0, 1.0));
                        if inner_size > brush_size * 0.04 {
                            let inner = projection.brush_outline(uv, inner_size, 64);
                            cursor_painter.add(Shape::line(
                                inner,
                                Stroke::new(1.0, cursor_color.gamma_multiply(0.65)),
                            ));
                        }
                    }
                } else if let Some(component) = app.masks.stack.selected_component() {
                    match &component.geometry {
                        MaskGeometry::Brush { size, .. } => {
                            let source_uv = projection.to_source(pointer);
                            if let Some(uv) = editable_source_uv(source_uv) {
                                let outline = projection.brush_outline(
                                    uv,
                                    zoom_scaled_brush_size(
                                        *size,
                                        app.preview.zoom,
                                        app.preferences.image_relative_brush_size,
                                    ),
                                    64,
                                );
                                painter.add(Shape::line(outline, Stroke::new(1.5, cursor_color)));
                            }
                        }
                        MaskGeometry::Object { brush_size, .. } => {
                            let source_uv = projection.to_source(pointer);
                            if let Some(uv) = editable_source_uv(source_uv) {
                                let outline = projection.brush_outline(
                                    uv,
                                    zoom_scaled_brush_size(
                                        *brush_size,
                                        app.preview.zoom,
                                        app.preferences.image_relative_brush_size,
                                    ),
                                    64,
                                );
                                painter.add(Shape::line(outline, Stroke::new(1.5, cursor_color)));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    pub(in crate::ui::preview) fn paint_coverage_texture(
        ui: &Ui,
        app: &mut CalibRawApp,
        layout: PreviewLayout,
        mask_index: usize,
        component_index: Option<usize>,
    ) {
        let PreviewLayout {
            visible_rect,
            source_width,
            source_height,
            ..
        } = layout;
        // Draw parametric-only overlays directly in corrected image space. This
        // preserves exact straight guides and avoids lens inversion on every drag.
        let corrected_space = app.masks.stack.masks.get(mask_index).is_some_and(|mask| {
            let is_parametric = |component: &crate::pipeline::MaskComponent| {
                matches!(
                    component.geometry,
                    MaskGeometry::Radial { .. }
                        | MaskGeometry::Linear { .. }
                        | MaskGeometry::Fullscreen
                )
            };
            component_index.map_or_else(
                || {
                    mask.components
                        .iter()
                        .filter(|component| component.enabled)
                        .all(is_parametric)
                },
                |index| mask.components.get(index).is_some_and(is_parametric),
            )
        });
        let lens_geometry = loaded_lens_geometry(app)
            .cloned()
            .filter(|_| !corrected_space);
        let visible_uv = if corrected_space {
            layout
                .projection(app.develop.geometry, None)
                .visible_source_uv(visible_rect)
        } else {
            app.preview.visible_uv
        };
        let primary_down = ui.input(|input| input.pointer.primary_down());
        let margin = app.masks.stack.raster_margin_pixels_for_layer(
            mask_index,
            component_index,
            source_width,
            source_height,
        );
        let region = mask_overlay_raster_region(
            overlay_raster_region(
                visible_uv,
                source_width,
                source_height,
                visible_rect,
                physical_pixels_per_point(ui.ctx()),
                margin,
            ),
            app.masks.interaction_dirty_layer,
            primary_down,
        );
        let key = (
            mask_index,
            component_index,
            app.masks.overlay_revision,
            region,
        );

        if app.masks.overlay_texture_key != Some(key) || app.masks.overlay_texture.is_none() {
            let extent = [region.texture_width, region.texture_height];
            let source_region = [
                region.source_x,
                region.source_y,
                region.source_width,
                region.source_height,
            ];
            let full_size = [source_width, source_height];
            let rgba = if let Some(component_index) = component_index {
                let coverage = app.masks.stack.rasterize_component_region(
                    mask_index,
                    component_index,
                    extent,
                    source_region,
                    full_size,
                    lens_geometry.as_deref(),
                );
                coverage_rgba(coverage, mask_component_color(component_index))
            } else {
                group_coverage_rgba(
                    &app.masks.stack,
                    mask_index,
                    extent,
                    source_region,
                    full_size,
                    lens_geometry.as_deref(),
                )
            };
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [
                    region.texture_width as usize,
                    region.texture_height as usize,
                ],
                &rgba,
            );
            if let Some(texture) = app.masks.overlay_texture.as_mut() {
                texture.set(image, egui::TextureOptions::LINEAR);
            } else {
                app.masks.overlay_texture = Some(ui.ctx().load_texture(
                    "selected-mask-coverage",
                    image,
                    egui::TextureOptions::LINEAR,
                ));
            }
            app.masks.overlay_texture_key = Some(key);
        }

        if let Some(texture) = &app.masks.overlay_texture {
            paint_final_geometry_overlay_texture(
                ui,
                texture.id(),
                layout.projection(app.develop.geometry, lens_geometry.as_deref()),
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                overlay_source_uv(region, source_width, source_height),
            );
        }
    }

    pub(in crate::ui::preview) fn paint_tool_hint(ui: &Ui, app: &CalibRawApp, preview_rect: Rect) {
        let Some(kind) = app.masks.active_tool else {
            return;
        };
        let text = match kind {
            MaskKind::Subject | MaskKind::Background if app.masks.subject_refinement_active => {
                match app.masks.brush_mode {
                    BrushMode::Paint => "Refine: paint subject",
                    BrushMode::Erase => "Refine: subtract subject / paint background",
                }
            }
            MaskKind::Brush => return,
            MaskKind::Object
                if app
                    .masks
                    .stack
                    .selected_component()
                    .is_some_and(|component| {
                        matches!(&component.geometry, MaskGeometry::Object { mask: None, .. })
                    }) =>
            {
                "Paint through the middle of the object part"
            }
            MaskKind::Radial
                if !app
                    .masks
                    .stack
                    .selected_component()
                    .is_some_and(|component| component.geometry.is_initialized()) =>
            {
                "Drag from the center to create a radial gradient"
            }
            MaskKind::Linear
                if !app
                    .masks
                    .stack
                    .selected_component()
                    .is_some_and(|component| component.geometry.is_initialized()) =>
            {
                "Drag across the image to create a linear gradient"
            }
            MaskKind::Path => {
                "Click to add points · drag to curve · Alt/Option-drag a point for handles"
            }
            MaskKind::ColorRange
                if !app
                    .masks
                    .stack
                    .selected_component()
                    .is_some_and(|component| {
                        matches!(
                            &component.geometry,
                            MaskGeometry::ColorRange { sampled: true, .. }
                        )
                    }) =>
            {
                "Drag on the image to sample a color"
            }
            _ => return,
        };
        let painter = ui.painter_at(preview_rect);
        let position = preview_rect.left_top() + egui::vec2(12.0, 12.0);
        painter.text(
            position,
            egui::Align2::LEFT_TOP,
            text,
            egui::FontId::proportional(13.0),
            Color32::WHITE,
        );
    }
}
