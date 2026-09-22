use super::*;

impl Sidebar {
    pub(super) fn apply_mask_properties_action(
        mask: &mut LocalMask,
        component_index: usize,
        action: super::super::adjustment_cards::CardAction,
    ) -> bool {
        use super::super::adjustment_cards::CardAction;
        match action {
            CardAction::None => return false,
            CardAction::Toggle => {}
            CardAction::Reset => {
                mask.enabled = true;
                mask.invert = false;
                mask.opacity = 1.0;
                if let Some(component) = mask.components.get_mut(component_index) {
                    component.enabled = true;
                    component.invert = false;
                    // Reset the property controls while retaining the selection itself.
                    let defaults = MaskGeometry::for_kind(component.kind);
                    let previous = std::mem::replace(&mut component.geometry, defaults);
                    match (&mut component.geometry, previous) {
                        (
                            MaskGeometry::Brush {
                                dabs,
                                stroke_starts,
                                ..
                            },
                            MaskGeometry::Brush {
                                dabs: saved,
                                stroke_starts: starts,
                                ..
                            },
                        ) => {
                            *dabs = saved;
                            *stroke_starts = starts;
                        }
                        (
                            MaskGeometry::Radial {
                                center,
                                radius,
                                rotation,
                                initialized,
                                ..
                            },
                            MaskGeometry::Radial {
                                center: c,
                                radius: r,
                                rotation: angle,
                                initialized: ready,
                                ..
                            },
                        ) => {
                            *center = c;
                            *radius = r;
                            *rotation = angle;
                            *initialized = ready;
                        }
                        (
                            MaskGeometry::Linear {
                                start,
                                end,
                                initialized,
                                ..
                            },
                            MaskGeometry::Linear {
                                start: a,
                                end: b,
                                initialized: ready,
                                ..
                            },
                        ) => {
                            *start = a;
                            *end = b;
                            *initialized = ready;
                        }
                        (
                            MaskGeometry::Path { points, .. },
                            MaskGeometry::Path { points: saved, .. },
                        ) => {
                            *points = saved;
                        }
                        (MaskGeometry::Ai { mask, .. }, MaskGeometry::Ai { mask: saved, .. }) => {
                            *mask = saved
                        }
                        (
                            MaskGeometry::Object { mask, strokes, .. },
                            MaskGeometry::Object {
                                mask: saved,
                                strokes: saved_strokes,
                                ..
                            },
                        ) => {
                            *mask = saved;
                            *strokes = saved_strokes;
                        }
                        (
                            MaskGeometry::LuminanceRange { source, .. },
                            MaskGeometry::LuminanceRange { source: saved, .. },
                        ) => *source = saved,
                        (
                            MaskGeometry::ColorRange {
                                source,
                                sample,
                                sampled,
                                ..
                            },
                            MaskGeometry::ColorRange {
                                source: saved,
                                sample: color,
                                sampled: ready,
                                ..
                            },
                        ) => {
                            *source = saved;
                            *sample = color;
                            *sampled = ready;
                        }
                        _ => {}
                    }
                }
            }
        }
        true
    }

    pub(crate) fn show_effect_components(
        ui: &mut Ui,
        components: &mut Vec<crate::pipeline::EffectComponent>,
        is_fullscreen_mask: bool,
    ) -> bool {
        let mut changed = false;
        let mut remove = None;
        for (index, component) in components.iter_mut().enumerate() {
            ui.push_id(index, |ui| {
                ui.horizontal(|ui| {
                    changed |= ui.checkbox(&mut component.enabled, "Enabled").changed();
                    if ui.button("Remove").clicked() {
                        remove = Some(index);
                    }
                });
                changed |= Self::show_effect_component_settings(ui, component, is_fullscreen_mask);
                crate::ui::theme::card_gap(ui);
            });
        }
        if let Some(index) = remove {
            components.remove(index);
            changed = true;
        }
        if components.len() < crate::pipeline::MAX_EFFECT_COMPONENTS {
            ui.menu_button(format!("{}  Add", egui_phosphor::regular::PLUS), |ui| {
                for category in MaskEffectCategory::ALL {
                    if !MaskEffect::ALL.iter().any(|effect| {
                        effect.category() == Some(category)
                            && !components
                                .iter()
                                .any(|component| component.effect == *effect)
                    }) {
                        continue;
                    }
                    ui.menu_button(category.label(), |ui| {
                        for effect in MaskEffect::ALL {
                            if effect.category() == Some(category)
                                && !components
                                    .iter()
                                    .any(|component| component.effect == effect)
                                && ui.button(effect.label()).clicked()
                            {
                                components.push(crate::pipeline::EffectComponent::new(effect));
                                changed = true;
                                ui.close();
                            }
                        }
                    });
                }
            });
        }
        changed
    }

    pub(super) fn show_effect_component_settings(
        ui: &mut Ui,
        component: &mut crate::pipeline::EffectComponent,
        is_fullscreen_mask: bool,
    ) -> bool {
        match component.effect {
            MaskEffect::Blur => {
                mask_effects::blur::show(ui, &mut component.settings.blur, &mut component.enabled)
            }
            MaskEffect::LensBlur => mask_effects::lens_blur::show(
                ui,
                &mut component.settings.lens_blur,
                &mut component.enabled,
            ),
            MaskEffect::MotionBlur => mask_effects::motion_blur::show(
                ui,
                &mut component.settings.motion_blur,
                &mut component.enabled,
            ),
            MaskEffect::RadialBlur => mask_effects::radial_blur::show(
                ui,
                &mut component.settings.radial_blur,
                &mut component.enabled,
            ),
            MaskEffect::TiltShift => mask_effects::tilt_shift::show(
                ui,
                &mut component.settings.tilt_shift,
                &mut component.enabled,
                is_fullscreen_mask,
            ),
            MaskEffect::EdgeGlow => mask_effects::edge_glow::show(
                ui,
                &mut component.settings.edge_glow,
                &mut component.enabled,
            ),
            MaskEffect::Glow => {
                mask_effects::glow::show(ui, &mut component.settings.glow, &mut component.enabled)
            }
            MaskEffect::LightRays => mask_effects::light_rays::show(
                ui,
                &mut component.settings.light_rays,
                &mut component.enabled,
            ),
            MaskEffect::Neon => {
                mask_effects::neon::show(ui, &mut component.settings.neon, &mut component.enabled)
            }
            MaskEffect::Pixelate => mask_effects::pixelate::show(
                ui,
                &mut component.settings.pixelate,
                &mut component.enabled,
            ),
            MaskEffect::Fog => {
                mask_effects::fog::show(ui, &mut component.settings.fog, &mut component.enabled)
            }
            MaskEffect::Smoke => {
                mask_effects::smoke::show(ui, &mut component.settings.smoke, &mut component.enabled)
            }
            MaskEffect::Adjustment => false,
        }
    }

    pub(super) fn is_plain_fullscreen_mask(mask: &LocalMask) -> bool {
        !mask.invert
            && matches!(
                mask.components.as_slice(),
                [component]
                    if component.enabled
                        && !component.invert
                        && component.kind == MaskKind::Fullscreen
            )
    }

    pub(super) fn apply_mask_geometry_change(
        ui: &Ui,
        app: &mut CalibRawApp,
        mask_index: usize,
        changed: bool,
    ) {
        if changed && ui.input(|input| input.pointer.primary_down()) {
            app.note_mask_geometry_interaction(mask_index);
        } else if changed {
            app.finish_mask_geometry_interaction();
            app.mark_mask_geometry_dirty(mask_index);
        } else if !ui.input(|input| input.pointer.primary_down()) {
            app.finish_mask_geometry_interaction();
        }
    }

    fn mask_grow_slider(ui: &mut Ui, grow: &mut f32) -> bool {
        adjustment_slider(
            ui,
            "Grow",
            grow,
            -1.0..=1.0,
            2,
            0.01,
            Some("Positive values expand the mask; negative values shrink it inward."),
        )
    }

    fn mask_feather_slider(
        ui: &mut Ui,
        label: &str,
        feather: &mut f32,
        range: std::ops::RangeInclusive<f32>,
        help: &str,
        reset: f32,
    ) -> bool {
        adjustment_slider_with_reset(ui, label, feather, range, 2, 0.01, Some(help), reset)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn show_vertical_mask_properties(
        ui: &mut Ui,
        mask: &mut crate::pipeline::LocalMask,
        component_index: usize,
        brush_mode: &mut BrushMode,
        subject_controls: (&mut bool, crate::ai_masks::BiRefNetQuality, bool),
        refinement_controls: (&mut bool, &mut f32, &mut f32, &mut f32, &mut bool),
        request_object: &mut bool,
    ) -> bool {
        let (request_subject, birefnet_quality, birefnet_quality_change_enabled) = subject_controls;
        let (
            refinement_active,
            refinement_size,
            refinement_feather,
            refinement_flow,
            clear_refinement,
        ) = refinement_controls;
        let mut opacity = mask.opacity;
        let mut geometry_changed = adjustment_slider_with_reset(
            ui,
            "Mask opacity",
            &mut opacity,
            0.0..=1.0,
            2,
            0.01,
            Some("Controls the strength of the entire mask before its selected type is applied."),
            1.0,
        );
        if geometry_changed {
            mask.set_opacity(opacity);
        }

        let Some(component) = mask.components.get_mut(component_index) else {
            return geometry_changed;
        };

        ui.add_space(crate::ui::theme::SPACE_XS);
        ui.scope(|ui| {
            let is_fullscreen = matches!(&component.geometry, MaskGeometry::Fullscreen);
            ui.horizontal_wrapped(|ui| {
                let component_name = ui.strong(component.name.as_str());
                if is_fullscreen {
                    component_name
                        .on_hover_text("Covers the complete image with uniform mask strength.");
                }
                if crate::ui::theme::toggle_button(ui, "Invert", component.invert).clicked() {
                    component.common.toggle_invert();
                    geometry_changed = true;
                }
                if component_index > 0 {
                    let mut combine = component.combine;
                    egui::ComboBox::from_id_salt("vertical-mask-combine")
                        .selected_text(combine.label())
                        .show_ui(ui, |ui| {
                            for mode in [
                                MaskCombineMode::Add,
                                MaskCombineMode::Subtract,
                                MaskCombineMode::Intersect,
                            ] {
                                ui.selectable_value(&mut combine, mode, mode.label());
                            }
                        });
                    geometry_changed |= component.set_combine(combine);
                }
            });

            match &mut component.geometry {
                MaskGeometry::Fullscreen => {}
                MaskGeometry::Brush {
                    size,
                    feather,
                    opacity_enabled,
                    opacity,
                    overlap_enabled,
                    stroke_starts,
                    dabs,
                } => {
                    ui.horizontal(|ui| {
                        let width = ((ui.available_width() - ui.spacing().item_spacing.x) * 0.5)
                            .max(1.0);
                        if crate::ui::theme::segmented_button(
                            ui,
                            "Brush",
                            *brush_mode == BrushMode::Paint,
                            width,
                        )
                        .clicked()
                        {
                            *brush_mode = BrushMode::Paint;
                        }
                        if crate::ui::theme::segmented_button(
                            ui,
                            "Eraser",
                            *brush_mode == BrushMode::Erase,
                            width,
                        )
                        .clicked()
                        {
                            *brush_mode = BrushMode::Erase;
                        }
                    });
                    geometry_changed |= adjustment_slider_with_reset(
                        ui,
                        "Size",
                        size,
                        0.0025..=0.25,
                        3,
                        0.0025,
                        Some("Brush stays the same size on screen; zoom in for finer image-space detail."),
                        0.055,
                    );
                    geometry_changed |= Self::mask_feather_slider(
                        ui,
                        "Feather",
                        feather,
                        0.0..=1.0,
                        "Softness from the brush core to its edge.",
                        0.55,
                    );
                    ui.horizontal(|ui| {
                        if crate::ui::theme::toggle_button(ui, "Opacity", *opacity_enabled)
                            .on_hover_text(
                                "Use the opacity setting for newly drawn brush and eraser strokes. \
                                 Disabled strokes always use 100% opacity.",
                            )
                            .clicked()
                        {
                            *opacity_enabled = !*opacity_enabled;
                            geometry_changed = true;
                        }
                        if crate::ui::theme::toggle_button(ui, "Overlapping", *overlap_enabled)
                            .on_hover_text(
                                "Allow separate brush strokes to build opacity where they overlap. \
                                 For example, 10% over 10% produces about 19% coverage.",
                            )
                            .clicked()
                        {
                            *overlap_enabled = !*overlap_enabled;
                            geometry_changed = true;
                        }
                    });
                    ui.add_enabled_ui(*opacity_enabled, |ui| {
                        geometry_changed |= adjustment_slider_with_reset(
                            ui,
                            "Stroke opacity",
                            opacity,
                            0.0..=1.0,
                            2,
                            0.01,
                            Some(
                                "Controls only newly drawn brush and eraser strokes. Existing \
                                 strokes and the whole-mask opacity are unchanged.",
                            ),
                            1.0,
                        );
                    });
                    if crate::ui::icons::phosphor_icon_button(
                        ui,
                        egui_phosphor::regular::ERASER,
                        crate::ui::theme::toolbar_icon_size(),
                        "Clear brush strokes",
                    )
                    .clicked()
                    {
                        dabs.clear();
                        stroke_starts.clear();
                        geometry_changed = true;
                    }
                }
                MaskGeometry::Radial { feather, .. } => {
                    geometry_changed |= Self::mask_feather_slider(
                        ui,
                        "Feather",
                        feather,
                        0.0..=1.0,
                        "Soft transition from the ellipse interior to its edge.",
                        0.55,
                    );
                }
                MaskGeometry::Linear { feather, .. } => {
                    geometry_changed |= Self::mask_feather_slider(
                        ui,
                        "Feather",
                        feather,
                        0.02..=1.0,
                        "Controls the width of the gradient transition.",
                        1.0,
                    );
                }
                MaskGeometry::Path { points, feather } => {
                    ui.label(concat!(
                        "Click to add polygon points. Click-drag while adding a point to create a ",
                        "smooth Bézier point. Drag anchors or handles to edit the path; ",
                        "Alt/Option-drag an anchor to create symmetric handles."
                    ));
                    geometry_changed |= Self::mask_feather_slider(
                        ui,
                        "Feather",
                        feather,
                        0.0..=1.0,
                        "Softens the freeform path boundary.",
                        0.0,
                    );
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .small_button("Straighten")
                            .on_hover_text("Convert all path points to straight corners")
                            .clicked()
                        {
                            for point in points.iter_mut() {
                                point.handle_in = [0.0, 0.0];
                                point.handle_out = [0.0, 0.0];
                            }
                            geometry_changed = true;
                        }
                        if ui
                            .small_button("Undo point")
                            .on_hover_text("Remove the last path point")
                            .clicked()
                        {
                            geometry_changed |= points.pop().is_some();
                        }
                        if ui
                            .small_button("Clear")
                            .on_hover_text("Clear path")
                            .clicked()
                        {
                            if !points.is_empty() {
                                points.clear();
                                geometry_changed = true;
                            }
                        }
                    });
                    ui.small(format!("{} point(s)", points.len()));
                }
                MaskGeometry::Ai {
                    mask: generated_mask,
                    grow,
                    feather,
                } => {
                    if crate::ui::theme::toggle_button(
                        ui,
                        if *refinement_active { "Done" } else { "Refine" },
                        *refinement_active,
                    )
                    .on_hover_text(
                        "Fine-tune the shared Subject / Not Subject boundary with a brush.",
                    )
                    .clicked()
                    {
                        *refinement_active = !*refinement_active;
                    }
                    if *refinement_active {
                        let action = Self::adjustment_card(ui, "Subject refinement", true, false, true, |ui| {
                            ui.horizontal(|ui| {
                                let width = ((ui.available_width()
                                    - ui.spacing().item_spacing.x)
                                    * 0.5)
                                    .max(1.0);
                                if crate::ui::theme::segmented_button(
                                    ui,
                                    "Add subject",
                                    *brush_mode == BrushMode::Paint,
                                    width,
                                )
                                .clicked()
                                {
                                    *brush_mode = BrushMode::Paint;
                                }
                                if crate::ui::theme::segmented_button(
                                    ui,
                                    "Subtract subject",
                                    *brush_mode == BrushMode::Erase,
                                    width,
                                )
                                .clicked()
                                {
                                    *brush_mode = BrushMode::Erase;
                                }
                            });
                            adjustment_slider_with_reset(
                                ui,
                                "Size",
                                refinement_size,
                                0.0025..=0.25,
                                3,
                                0.0025,
                                Some(
                                    "Brush stays the same size on screen; zoom in for finer image-space detail.",
                                ),
                                0.035,
                            );
                            Self::mask_feather_slider(
                                ui,
                                "Feather",
                                refinement_feather,
                                0.0..=1.0,
                                "Softness of newly painted refinement strokes.",
                                0.55,
                            );
                            adjustment_slider_with_reset(
                                ui,
                                "Flow / opacity",
                                refinement_flow,
                                0.01..=1.0,
                                2,
                                0.01,
                                Some("Strength captured by newly painted add/subtract strokes."),
                                1.0,
                            );
                            if crate::ui::icons::phosphor_icon_button(
                                ui,
                                egui_phosphor::regular::ERASER,
                                    crate::ui::theme::toolbar_icon_size(),
                                "Clear subject refinement",
                            )
                            .clicked()
                            {
                                *clear_refinement = true;
                            }
                        });
                        match action {
                            super::super::adjustment_cards::CardAction::None => {},
                            super::super::adjustment_cards::CardAction::Toggle => {},
                            super::super::adjustment_cards::CardAction::Reset => {
                                let defaults = crate::pipeline::SubjectRefinement::default();
                                *refinement_size = defaults.size;
                                *refinement_feather = defaults.feather;
                                *refinement_flow = defaults.flow;
                                *clear_refinement = true;
                            }
                        }
                    }
                    if generated_mask.is_none() {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(format!(
                                "Generate in {} quality",
                                birefnet_quality.label()
                            ));
                            if ui
                                .add_enabled(
                                    birefnet_quality_change_enabled,
                                    egui::Button::new("Generate subject mask"),
                                )
                                .clicked()
                            {
                                *request_subject = true;
                            }
                            if !birefnet_quality_change_enabled {
                                ui.spinner();
                            }
                        });
                    }
                    geometry_changed |= Self::mask_grow_slider(ui, grow);
                    geometry_changed |= Self::mask_feather_slider(
                        ui,
                        "Feather",
                        feather,
                        0.0..=1.0,
                        "Softens the BiRefNet subject boundary.",
                        0.0,
                    );
                }
                MaskGeometry::Object {
                    mask: generated_mask,
                    grow,
                    feather,
                    brush_size,
                    edge_refine,
                    strokes,
                } => {
                    *brush_mode = BrushMode::Paint;
                    ui.label(if generated_mask.is_some() {
                        "Draw again on the image to replace this object selection from scratch."
                    } else {
                        "Paint through the middle of the object part you want to select."
                    });
                    ui.strong("Selection brush");
                    geometry_changed |= adjustment_slider_with_reset(
                        ui,
                        "Size",
                        brush_size,
                        0.0025..=0.25,
                        3,
                        0.0025,
                        Some("Controls the hard-edged selection brush. Its on-screen size stays constant while zooming for finer detail."),
                        0.055,
                    );
                    ui.add_space(crate::ui::theme::SPACE_XS);
                    geometry_changed |= Self::mask_grow_slider(ui, grow);
                    geometry_changed |= Self::mask_feather_slider(
                        ui,
                        "Mask feather",
                        feather,
                        0.0..=1.0,
                        "Softens the final object mask after SAM selection.",
                        0.0,
                    );
                    let refine_changed = adjustment_slider_with_reset(
                        ui,
                        "Edge refine",
                        edge_refine,
                        0.0..=1.0,
                        2,
                        0.01,
                        Some("Aligns uncertain SAM boundaries to local image edges."),
                        0.55,
                    );
                    geometry_changed |= refine_changed;
                    if refine_changed && !strokes.is_empty() {
                        *request_object = true;
                    }
                    ui.horizontal_wrapped(|ui| {
                        if crate::ui::icons::phosphor_icon_button(
                            ui,
                            egui_phosphor::regular::ARROW_CLOCKWISE,
                            crate::ui::theme::toolbar_icon_size(),
                            "Recalculate object selection",
                        )
                        .clicked()
                        {
                            *request_object = true;
                        }
                        if crate::ui::icons::phosphor_icon_button(
                            ui,
                            egui_phosphor::regular::X,
                            crate::ui::theme::toolbar_icon_size(),
                            "Clear object selection",
                        )
                        .clicked()
                        {
                            strokes.clear();
                            *generated_mask = None;
                            geometry_changed = true;
                        }
                    });
                    ui.small(format!("{} selection stroke(s)", strokes.len()));
                }
                MaskGeometry::LuminanceRange {
                    low,
                    high,
                    grow,
                    feather,
                    ..
                } => {
                    geometry_changed |= adjustment_slider_with_reset(
                        ui,
                        "Range low",
                        low,
                        0.0..=1.0,
                        2,
                        0.01,
                        Some("Lowest included scene luminance."),
                        0.2,
                    );
                    geometry_changed |= adjustment_slider_with_reset(
                        ui,
                        "Range high",
                        high,
                        0.0..=1.0,
                        2,
                        0.01,
                        Some("Highest included scene luminance."),
                        0.8,
                    );
                    geometry_changed |= Self::mask_grow_slider(ui, grow);
                    geometry_changed |= Self::mask_feather_slider(
                        ui,
                        "Range feather",
                        feather,
                        0.0..=1.0,
                        "Softens both luminance-range boundaries.",
                        0.15,
                    );
                }
                MaskGeometry::ColorRange {
                    tolerance,
                    grow,
                    feather,
                    sampled,
                    ..
                } => {
                    ui.label(if *sampled {
                        "Drag on the image to choose another color."
                    } else {
                        "Drag on the image to sample a color."
                    });
                    geometry_changed |= adjustment_slider_with_reset(
                        ui,
                        "Tolerance",
                        tolerance,
                        0.005..=1.0,
                        3,
                        0.005,
                        Some("Expands the selected color region in perceptual OkLab space."),
                        0.18,
                    );
                    geometry_changed |= Self::mask_grow_slider(ui, grow);
                    geometry_changed |= Self::mask_feather_slider(
                        ui,
                        "Color feather",
                        feather,
                        0.0..=1.0,
                        "Softens the color-distance cutoff.",
                        0.12,
                    );
                }
                MaskGeometry::Placeholder => {
                    ui.label("This mask type is not implemented yet.");
                }
            }
        });

        geometry_changed
    }
}

#[cfg(test)]
mod card_tests {
    use super::*;

    #[test]
    fn properties_reset_keeps_painted_selection_and_local_edits() {
        let mut mask = LocalMask::new(MaskKind::Brush, 1);
        mask.adjustments.exposure = 1.5;
        mask.opacity = 0.2;
        let dab = crate::pipeline::BrushDab::default();
        if let MaskGeometry::Brush { size, dabs, .. } = &mut mask.components[0].geometry {
            *size = 0.2;
            dabs.push(dab);
        }
        Sidebar::apply_mask_properties_action(
            &mut mask,
            0,
            super::super::super::adjustment_cards::CardAction::Reset,
        );
        assert_eq!(mask.opacity, 1.0);
        assert_eq!(mask.adjustments.exposure, 1.5);
        match &mask.components[0].geometry {
            MaskGeometry::Brush { size, dabs, .. } => {
                assert_eq!(*size, 0.055);
                assert_eq!(dabs, &[dab]);
            }
            _ => panic!("brush selection changed type"),
        }
    }

    #[test]
    fn properties_reset_keeps_freeform_path_geometry() {
        let mut mask = LocalMask::new(MaskKind::Path, 1);
        let saved = vec![
            crate::pipeline::PathPoint::corner([0.2, 0.2]),
            crate::pipeline::PathPoint {
                position: [0.8, 0.2],
                handle_in: [-0.1, 0.0],
                handle_out: [0.1, 0.0],
            },
            crate::pipeline::PathPoint::corner([0.5, 0.8]),
        ];
        if let MaskGeometry::Path { points, feather } = &mut mask.components[0].geometry {
            *points = saved.clone();
            *feather = 0.75;
        }

        Sidebar::apply_mask_properties_action(
            &mut mask,
            0,
            super::super::super::adjustment_cards::CardAction::Reset,
        );

        match &mask.components[0].geometry {
            MaskGeometry::Path { points, feather } => {
                assert_eq!(points, &saved);
                assert_eq!(*feather, 0.0);
            }
            _ => panic!("path selection changed type"),
        }
    }
}
