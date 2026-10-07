use super::*;
use crate::app::{MaskPropertiesControls, MaskPropertyAction};
use crate::ui::components::luminance_range_slider::{luminance_range_slider, LuminanceRange};

impl Sidebar {
    pub(crate) fn effect_creation_menu(
        ui: &mut Ui,
        components: &[crate::pipeline::EffectComponent],
    ) -> Option<MaskEffect> {
        let mut selected = None;
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
                        && moduwu_design::menu_item(ui, true, effect.label()).clicked()
                    {
                        selected = Some(effect);
                        ui.close();
                    }
                }
            });
        }
        selected
    }

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
                            MaskGeometry::DepthRange { depth, .. },
                            MaskGeometry::DepthRange { depth: saved, .. },
                        ) => {
                            *depth = saved;
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
        frame: &mask_effects::EffectFrame,
    ) -> bool {
        let mut changed = false;
        let mut remove = None;
        for (index, component) in components.iter_mut().enumerate() {
            ui.push_id(index, |ui| {
                let mut remove_component = false;
                changed |= Self::show_effect_component_settings(
                    ui,
                    component,
                    &mut remove_component,
                    is_fullscreen_mask,
                    frame,
                );
                if remove_component {
                    remove = Some(index);
                }
            });
        }
        if let Some(index) = remove {
            components.remove(index);
            changed = true;
        }
        if components.len() < crate::pipeline::MAX_EFFECT_COMPONENTS {
            ui.menu_button(format!("{}  Add", egui_phosphor::regular::PLUS), |ui| {
                if let Some(effect) = Self::effect_creation_menu(ui, components) {
                    components.push(crate::pipeline::EffectComponent::new(effect));
                    changed = true;
                }
            });
        }
        changed
    }

    pub(crate) fn show_selected_effect_component(
        ui: &mut Ui,
        components: &mut Vec<crate::pipeline::EffectComponent>,
        selection: &mut Option<MaskEffect>,
        is_fullscreen_mask: bool,
        frame: &mask_effects::EffectFrame,
    ) -> bool {
        let Some(effect) = *selection else {
            return false;
        };
        let Some(index) = components
            .iter()
            .position(|component| component.effect == effect)
        else {
            *selection = None;
            return false;
        };
        let mut remove = false;
        let changed = ui
            .push_id(index, |ui| {
                Self::show_effect_component_settings(
                    ui,
                    &mut components[index],
                    &mut remove,
                    is_fullscreen_mask,
                    frame,
                )
            })
            .inner;
        if remove {
            components.remove(index);
            *selection = None;
        }
        changed || remove
    }

    pub(super) fn show_effect_component_settings(
        ui: &mut Ui,
        component: &mut crate::pipeline::EffectComponent,
        remove: &mut bool,
        is_fullscreen_mask: bool,
        frame: &mask_effects::EffectFrame,
    ) -> bool {
        match component.effect {
            MaskEffect::Blur => mask_effects::blur::show(
                ui,
                &mut component.settings.blur,
                &mut component.enabled,
                remove,
            ),
            MaskEffect::LensBlur => mask_effects::lens_blur::show(
                ui,
                &mut component.settings.lens_blur,
                &mut component.enabled,
                remove,
            ),
            MaskEffect::MotionBlur => mask_effects::motion_blur::show(
                ui,
                &mut component.settings.motion_blur,
                &mut component.enabled,
                remove,
            ),
            MaskEffect::RadialBlur => mask_effects::radial_blur::show(
                ui,
                &mut component.settings.radial_blur,
                &mut component.enabled,
                remove,
                frame,
            ),
            MaskEffect::TiltShift => mask_effects::tilt_shift::show(
                ui,
                &mut component.settings.tilt_shift,
                &mut component.enabled,
                remove,
                frame,
                is_fullscreen_mask,
            ),
            MaskEffect::EdgeGlow => mask_effects::edge_glow::show(
                ui,
                &mut component.settings.edge_glow,
                &mut component.enabled,
                remove,
            ),
            MaskEffect::Glow => mask_effects::glow::show(
                ui,
                &mut component.settings.glow,
                &mut component.enabled,
                remove,
            ),
            MaskEffect::LightRays => mask_effects::light_rays::show(
                ui,
                &mut component.settings.light_rays,
                &mut component.enabled,
                remove,
                frame,
            ),
            MaskEffect::Relight => mask_effects::relight::show(
                ui,
                &mut component.settings.relight,
                &mut component.enabled,
                remove,
                frame,
            ),
            MaskEffect::Neon => mask_effects::neon::show(
                ui,
                &mut component.settings.neon,
                &mut component.enabled,
                remove,
            ),
            MaskEffect::Pixelate => mask_effects::pixelate::show(
                ui,
                &mut component.settings.pixelate,
                &mut component.enabled,
                remove,
            ),
            MaskEffect::Fog => mask_effects::fog::show(
                ui,
                &mut component.settings.fog,
                &mut component.enabled,
                remove,
            ),
            MaskEffect::Smoke => mask_effects::smoke::show(
                ui,
                &mut component.settings.smoke,
                &mut component.enabled,
                remove,
            ),
            MaskEffect::Grain => mask_effects::grain::show(
                ui,
                &mut component.settings.grain,
                &mut component.enabled,
                remove,
            ),
            MaskEffect::Halation => mask_effects::halation::show(
                ui,
                &mut component.settings.halation,
                &mut component.enabled,
                remove,
            ),
            MaskEffect::Vignette => mask_effects::vignette::show(
                ui,
                &mut component.settings.vignette,
                &mut component.enabled,
                remove,
                frame,
            ),
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
        AdjustmentSlider::new("Grow", grow, -1.0..=1.0)
            .decimals(2)
            .step(0.01)
            .hover_text("Positive values expand the mask; negative values shrink it inward.")
            .show(ui)
    }

    fn mask_feather_slider(
        ui: &mut Ui,
        label: &str,
        feather: &mut f32,
        range: std::ops::RangeInclusive<f32>,
        help: &str,
        reset: f32,
    ) -> bool {
        AdjustmentSlider::new(label, feather, range)
            .decimals(2)
            .step(0.01)
            .hover_text(help)
            .reset_to(reset)
            .show(ui)
    }

    /// The properties of the selected component. Returns the edits, in the
    /// order they apply; `controls` is the tool state at the start of the frame.
    pub(super) fn show_vertical_mask_properties(
        ui: &mut Ui,
        mask: &LocalMask,
        component_index: usize,
        controls: &MaskPropertiesControls,
    ) -> Vec<MaskPropertyAction> {
        let mut edits = PropertyEdits {
            actions: Vec::new(),
            controls: *controls,
        };
        let mut opacity = mask.opacity;
        if AdjustmentSlider::new("Mask opacity", &mut opacity, 0.0..=1.0)
            .decimals(2)
            .step(0.01)
            .hover_text(
                "Controls the strength of the entire mask before its selected type is applied.",
            )
            .reset_to(1.0)
            .show(ui)
        {
            edits.push(MaskPropertyAction::SetOpacity(opacity));
        }

        let Some(component) = mask.components.get(component_index) else {
            return edits.actions;
        };

        ui.add_space(moduwu_design::SPACE_XS);
        ui.scope(|ui| {
            Self::component_header(ui, component, component_index, &mut edits);
            let is_sky = component.kind == MaskKind::Sky;
            match &component.geometry {
                MaskGeometry::Fullscreen => {}
                MaskGeometry::Brush { .. } => {
                    Self::brush_properties(ui, &component.geometry, &mut edits);
                }
                MaskGeometry::Radial { feather, .. } => {
                    edits.feather(Self::mask_feather_edit(
                        ui,
                        "Feather",
                        *feather,
                        0.0..=1.0,
                        "Soft transition from the ellipse interior to its edge.",
                        0.55,
                    ));
                }
                MaskGeometry::Linear { feather, .. } => {
                    edits.feather(Self::mask_feather_edit(
                        ui,
                        "Feather",
                        *feather,
                        0.02..=1.0,
                        "Controls the width of the gradient transition.",
                        1.0,
                    ));
                }
                MaskGeometry::Path {
                    points,
                    grow,
                    feather,
                } => Self::path_properties(ui, points.len(), *grow, *feather, &mut edits),
                MaskGeometry::Ai {
                    mask: generated_mask,
                    grow,
                    feather,
                } => {
                    if !is_sky {
                        Self::subject_refinement_controls(ui, &mut edits);
                    }
                    if generated_mask.is_none() {
                        Self::generate_mask_row(ui, is_sky, &mut edits);
                    }
                    edits.grow(Self::mask_grow_edit(ui, *grow));
                    edits.feather(Self::mask_feather_edit(
                        ui,
                        "Feather",
                        *feather,
                        0.0..=1.0,
                        "Softens the selection edge while keeping the selected interior solid.",
                        0.0,
                    ));
                }
                MaskGeometry::Object { .. } => {
                    Self::object_properties(ui, &component.geometry, &mut edits);
                }
                MaskGeometry::LuminanceRange {
                    low,
                    high,
                    grow,
                    feather,
                    high_feather,
                    ..
                } => Self::luminance_range_properties(
                    ui,
                    LuminanceRange {
                        low: *low,
                        high: *high,
                        low_feather: *feather,
                        high_feather: high_feather.unwrap_or(*feather),
                    },
                    *grow,
                    &mut edits,
                ),
                MaskGeometry::ColorRange {
                    tolerance,
                    grow,
                    feather,
                    sampled,
                    ..
                } => Self::color_range_properties(
                    ui, *sampled, *tolerance, *grow, *feather, &mut edits,
                ),
                MaskGeometry::DepthRange { depth, range } => {
                    Self::depth_range_properties(ui, depth.is_some(), *range, &mut edits);
                }
                MaskGeometry::Placeholder => {
                    ui.label("This mask type is not implemented yet.");
                }
            }
        });

        edits.actions
    }

    /// The component name, its Invert toggle and, after the base, its combine mode.
    fn component_header(
        ui: &mut Ui,
        component: &MaskComponent,
        component_index: usize,
        edits: &mut PropertyEdits,
    ) {
        let is_fullscreen = matches!(&component.geometry, MaskGeometry::Fullscreen);
        ui.horizontal_wrapped(|ui| {
            let component_name = ui.strong(component.name.as_str());
            if is_fullscreen {
                component_name
                    .on_hover_text("Covers the complete image with uniform mask strength.");
            }
            if moduwu_design::toggle_button(ui, "Invert", component.invert).clicked() {
                edits.push(MaskPropertyAction::ToggleInvert);
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
                if combine != component.combine {
                    edits.push(MaskPropertyAction::SetCombine(combine));
                }
            }
        });
    }

    fn brush_properties(ui: &mut Ui, geometry: &MaskGeometry, edits: &mut PropertyEdits) {
        let MaskGeometry::Brush {
            size,
            feather,
            opacity_enabled,
            opacity,
            overlap_enabled,
            ..
        } = geometry
        else {
            return;
        };
        Self::brush_mode_selector(ui, edits, "Brush", "Eraser");
        let mut size = *size;
        if AdjustmentSlider::new("Size", &mut size, 0.0025..=0.25)
            .decimals(3)
            .step(0.0025)
            .hover_text(SCREEN_SIZED_BRUSH_HELP)
            .reset_to(0.055)
            .show(ui)
        {
            edits.push(MaskPropertyAction::SetBrushSize(size));
        }
        if let Some(feather) = Self::mask_feather_edit(
            ui,
            "Feather",
            *feather,
            0.0..=1.0,
            "Softness from the brush core to its edge.",
            0.55,
        ) {
            edits.push(MaskPropertyAction::SetBrushFeather(feather));
        }
        // Toggling opacity enables the stroke opacity slider in the same frame.
        let mut opacity_enabled = *opacity_enabled;
        ui.horizontal(|ui| {
            if moduwu_design::toggle_button(ui, "Opacity", opacity_enabled)
                .on_hover_text(
                    "Use the opacity setting for newly drawn brush and eraser strokes. \
                     Disabled strokes always use 100% opacity.",
                )
                .clicked()
            {
                opacity_enabled = !opacity_enabled;
                edits.push(MaskPropertyAction::ToggleBrushOpacity);
            }
            if moduwu_design::toggle_button(ui, "Overlapping", *overlap_enabled)
                .on_hover_text(
                    "Allow separate brush strokes to build opacity where they overlap. \
                     For example, 10% over 10% produces about 19% coverage.",
                )
                .clicked()
            {
                edits.push(MaskPropertyAction::ToggleBrushOverlap);
            }
        });
        ui.add_enabled_ui(opacity_enabled, |ui| {
            let mut opacity = *opacity;
            if AdjustmentSlider::new("Stroke opacity", &mut opacity, 0.0..=1.0)
                .decimals(2)
                .step(0.01)
                .hover_text(
                    "Controls only newly drawn brush and eraser strokes. Existing \
                 strokes and the whole-mask opacity are unchanged.",
                )
                .reset_to(1.0)
                .show(ui)
            {
                edits.push(MaskPropertyAction::SetBrushStrokeOpacity(opacity));
            }
        });
        if moduwu_design::icon_button(
            ui,
            egui_phosphor::regular::ERASER,
            moduwu_design::toolbar_icon_size(),
            "Clear brush strokes",
        )
        .clicked()
        {
            edits.push(MaskPropertyAction::ClearBrushStrokes);
        }
    }

    fn path_properties(
        ui: &mut Ui,
        point_count: usize,
        grow: f32,
        feather: f32,
        edits: &mut PropertyEdits,
    ) {
        ui.label(concat!(
            "Click to add polygon points. Click-drag while adding a point to create a ",
            "smooth Bézier point. Drag anchors or handles to edit the path; ",
            "Alt/Option-drag an anchor to create symmetric handles."
        ));
        edits.grow(Self::mask_grow_edit(ui, grow));
        edits.feather(Self::mask_feather_edit(
            ui,
            "Feather",
            feather,
            0.0..=1.0,
            "Softens only the inside of the freeform path edge.",
            0.0,
        ));
        // The count below reflects this frame's edits.
        let mut point_count = point_count;
        ui.horizontal_wrapped(|ui| {
            if ui
                .small_button("Straighten")
                .on_hover_text("Convert all path points to straight corners")
                .clicked()
            {
                edits.push(MaskPropertyAction::StraightenPath);
            }
            if ui
                .small_button("Undo point")
                .on_hover_text("Remove the last path point")
                .clicked()
            {
                edits.push(MaskPropertyAction::RemoveLastPathPoint);
                point_count = point_count.saturating_sub(1);
            }
            if ui
                .small_button("Clear")
                .on_hover_text("Clear path")
                .clicked()
                && point_count > 0
            {
                edits.push(MaskPropertyAction::ClearPath);
                point_count = 0;
            }
        });
        ui.small(format!("{point_count} point(s)"));
    }

    fn object_properties(ui: &mut Ui, geometry: &MaskGeometry, edits: &mut PropertyEdits) {
        let MaskGeometry::Object {
            mask: generated_mask,
            grow,
            feather,
            brush_size,
            edge_refine,
            strokes,
        } = geometry
        else {
            return;
        };
        edits.set_brush_mode(BrushMode::Paint);
        ui.label(if generated_mask.is_some() {
            "Draw again on the image to replace this object selection from scratch."
        } else {
            "Paint through the middle of the object part you want to select."
        });
        ui.strong("Selection brush");
        let mut brush_size = *brush_size;
        if AdjustmentSlider::new("Size", &mut brush_size, 0.0025..=0.25)
            .decimals(3)
            .step(0.0025)
            .hover_text(
                "Controls the hard-edged selection brush. Its on-screen size stays \
                 constant while zooming for finer detail.",
            )
            .reset_to(0.055)
            .show(ui)
        {
            edits.push(MaskPropertyAction::SetObjectBrushSize(brush_size));
        }
        ui.add_space(moduwu_design::SPACE_XS);
        edits.grow(Self::mask_grow_edit(ui, *grow));
        edits.feather(Self::mask_feather_edit(
            ui,
            "Mask feather",
            *feather,
            0.0..=1.0,
            "Softens the final object mask after SAM selection.",
            0.0,
        ));
        let mut edge_refine = *edge_refine;
        if AdjustmentSlider::new("Edge refine", &mut edge_refine, 0.0..=1.0)
            .decimals(2)
            .step(0.01)
            .hover_text("Aligns uncertain SAM boundaries to local image edges.")
            .reset_to(0.55)
            .show(ui)
        {
            edits.push(MaskPropertyAction::SetEdgeRefine(edge_refine));
            if !strokes.is_empty() {
                edits.push(MaskPropertyAction::RequestObject);
            }
        }
        // The count below reflects a clear in this frame.
        let mut stroke_count = strokes.len();
        ui.horizontal_wrapped(|ui| {
            if moduwu_design::icon_button(
                ui,
                egui_phosphor::regular::ARROW_CLOCKWISE,
                moduwu_design::toolbar_icon_size(),
                "Recalculate object selection",
            )
            .clicked()
            {
                edits.push(MaskPropertyAction::RequestObject);
            }
            if moduwu_design::icon_button(
                ui,
                egui_phosphor::regular::X,
                moduwu_design::toolbar_icon_size(),
                "Clear object selection",
            )
            .clicked()
            {
                edits.push(MaskPropertyAction::ClearObjectSelection);
                stroke_count = 0;
            }
        });
        ui.small(format!("{stroke_count} selection stroke(s)"));
    }

    fn luminance_range_properties(
        ui: &mut Ui,
        range: LuminanceRange,
        grow: f32,
        edits: &mut PropertyEdits,
    ) {
        let mut edited = range;
        if luminance_range_slider(ui, &mut edited) {
            if edited.low != range.low {
                edits.push(MaskPropertyAction::SetLuminanceLow(edited.low));
            }
            if edited.high != range.high {
                edits.push(MaskPropertyAction::SetLuminanceHigh(edited.high));
            }
            if edited.low_feather != range.low_feather {
                edits.feather(Some(edited.low_feather));
            }
            if edited.high_feather != range.high_feather {
                edits.push(MaskPropertyAction::SetLuminanceHighFeather(
                    edited.high_feather,
                ));
            }
        }
        edits.grow(Self::mask_grow_edit(ui, grow));
    }

    fn color_range_properties(
        ui: &mut Ui,
        sampled: bool,
        tolerance: f32,
        grow: f32,
        feather: f32,
        edits: &mut PropertyEdits,
    ) {
        ui.label(if sampled {
            "Drag on the image to choose another color."
        } else {
            "Drag on the image to sample a color."
        });
        let mut tolerance = tolerance;
        if AdjustmentSlider::new("Tolerance", &mut tolerance, 0.005..=1.0)
            .decimals(3)
            .step(0.005)
            .hover_text("Expands the selected color region in perceptual OkLab space.")
            .reset_to(0.18)
            .show(ui)
        {
            edits.push(MaskPropertyAction::SetColorTolerance(tolerance));
        }
        edits.grow(Self::mask_grow_edit(ui, grow));
        edits.feather(Self::mask_feather_edit(
            ui,
            "Color feather",
            feather,
            0.0..=1.0,
            "Softens the color-distance cutoff.",
            0.12,
        ));
    }

    fn depth_range_properties(
        ui: &mut Ui,
        has_depth: bool,
        range: crate::pipeline::DepthRangeSettings,
        edits: &mut PropertyEdits,
    ) {
        ui.label(if has_depth {
            "Select a range of relative depth, from near (0) to far (1)."
        } else {
            "Generate a depth map to select by distance."
        });
        let label = if has_depth {
            "Regenerate depth map"
        } else {
            "Generate depth map"
        };
        if moduwu_design::secondary_button(ui, label).clicked() {
            edits.push(MaskPropertyAction::RequestGeneration);
        }
        let mut range = range;
        if crate::ui::components::depth_range_slider::depth_range_slider(ui, &mut range) {
            edits.push(MaskPropertyAction::SetDepthRange(range));
        }
    }

    fn mask_grow_edit(ui: &mut Ui, grow: f32) -> Option<f32> {
        let mut grow = grow;
        Self::mask_grow_slider(ui, &mut grow).then_some(grow)
    }

    fn mask_feather_edit(
        ui: &mut Ui,
        label: &str,
        feather: f32,
        range: std::ops::RangeInclusive<f32>,
        help: &str,
        reset: f32,
    ) -> Option<f32> {
        let mut feather = feather;
        Self::mask_feather_slider(ui, label, &mut feather, range, help, reset).then_some(feather)
    }

    fn brush_mode_selector(
        ui: &mut Ui,
        edits: &mut PropertyEdits,
        paint_label: &str,
        erase_label: &str,
    ) {
        ui.horizontal(|ui| {
            let width = ((ui.available_width() - ui.spacing().item_spacing.x) * 0.5).max(1.0);
            for (mode, label) in [
                (BrushMode::Paint, paint_label),
                (BrushMode::Erase, erase_label),
            ] {
                if moduwu_design::segmented_button(
                    ui,
                    label,
                    edits.controls.brush_mode == mode,
                    width,
                )
                .clicked()
                {
                    edits.set_brush_mode(mode);
                }
            }
        });
    }

    /// The Refine toggle and, while active, the brush settings for editing the
    /// boundary shared by Subject and Background masks.
    fn subject_refinement_controls(ui: &mut Ui, edits: &mut PropertyEdits) {
        let active = edits.controls.refinement_active;
        let toggle_label = if active { "Done" } else { "Refine" };
        if moduwu_design::toggle_button(ui, toggle_label, active)
            .on_hover_text("Fine-tune the shared Subject / Background boundary with a brush.")
            .clicked()
        {
            edits.controls.refinement_active = !active;
            edits.push(MaskPropertyAction::SetRefinementActive(!active));
        }
        if !edits.controls.refinement_active {
            return;
        }
        let action = Self::adjustment_card(ui, "Subject refinement", true, false, true, |ui| {
            Self::brush_mode_selector(ui, edits, "Add subject", "Subtract subject");
            let mut size = edits.controls.refinement_size;
            if AdjustmentSlider::new("Size", &mut size, 0.0025..=0.25)
                .decimals(3)
                .step(0.0025)
                .hover_text(SCREEN_SIZED_BRUSH_HELP)
                .reset_to(0.035)
                .show(ui)
            {
                edits.set_refinement_size(size);
            }
            let mut feather = edits.controls.refinement_feather;
            if Self::mask_feather_slider(
                ui,
                "Feather",
                &mut feather,
                0.0..=1.0,
                "Softness of newly painted refinement strokes.",
                0.55,
            ) {
                edits.set_refinement_feather(feather);
            }
            let mut flow = edits.controls.refinement_flow;
            if AdjustmentSlider::new("Flow / opacity", &mut flow, 0.01..=1.0)
                .decimals(2)
                .step(0.01)
                .hover_text("Strength captured by newly painted add/subtract strokes.")
                .reset_to(1.0)
                .show(ui)
            {
                edits.set_refinement_flow(flow);
            }
            if moduwu_design::icon_button(
                ui,
                egui_phosphor::regular::ERASER,
                moduwu_design::toolbar_icon_size(),
                "Clear subject refinement",
            )
            .clicked()
            {
                edits.push(MaskPropertyAction::ClearRefinement);
            }
        });
        if matches!(action, super::super::adjustment_cards::CardAction::Reset) {
            let defaults = crate::pipeline::SubjectRefinement::default();
            edits.set_refinement_size(defaults.size);
            edits.set_refinement_feather(defaults.feather);
            edits.set_refinement_flow(defaults.flow);
            edits.push(MaskPropertyAction::ClearRefinement);
        }
    }

    fn generate_mask_row(ui: &mut Ui, is_sky: bool, edits: &mut PropertyEdits) {
        let controls = edits.controls;
        ui.horizontal_wrapped(|ui| {
            if is_sky {
                ui.label("Generate with SkySeg U2Net");
            } else {
                ui.label(format!(
                    "Generate in {} quality",
                    controls.birefnet_quality.label()
                ));
            }
            let label = if is_sky {
                "Generate sky mask"
            } else {
                "Generate subject mask"
            };
            if moduwu_design::secondary_button_enabled(ui, controls.generation_idle, label)
                .clicked()
            {
                edits.push(MaskPropertyAction::RequestGeneration);
            }
            if !controls.generation_idle {
                ui.spinner();
            }
        });
    }
}

/// The panel's edits, plus a draft of the tool state so controls drawn later
/// in the frame show earlier edits.
struct PropertyEdits {
    actions: Vec<MaskPropertyAction>,
    controls: MaskPropertiesControls,
}

impl PropertyEdits {
    fn push(&mut self, action: MaskPropertyAction) {
        self.actions.push(action);
    }

    fn feather(&mut self, feather: Option<f32>) {
        if let Some(feather) = feather {
            self.push(MaskPropertyAction::SetFeather(feather));
        }
    }

    fn grow(&mut self, grow: Option<f32>) {
        if let Some(grow) = grow {
            self.push(MaskPropertyAction::SetGrow(grow));
        }
    }

    fn set_brush_mode(&mut self, mode: BrushMode) {
        if self.controls.brush_mode != mode {
            self.controls.brush_mode = mode;
            self.push(MaskPropertyAction::SetBrushMode(mode));
        }
    }

    fn set_refinement_size(&mut self, size: f32) {
        self.controls.refinement_size = size;
        self.push(MaskPropertyAction::SetRefinementSize(size));
    }

    fn set_refinement_feather(&mut self, feather: f32) {
        self.controls.refinement_feather = feather;
        self.push(MaskPropertyAction::SetRefinementFeather(feather));
    }

    fn set_refinement_flow(&mut self, flow: f32) {
        self.controls.refinement_flow = flow;
        self.push(MaskPropertyAction::SetRefinementFlow(flow));
    }
}

const SCREEN_SIZED_BRUSH_HELP: &str =
    "Brush stays the same size on screen; zoom in for finer image-space detail.";

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
        if let MaskGeometry::Path {
            points,
            grow,
            feather,
        } = &mut mask.components[0].geometry
        {
            *points = saved.clone();
            *grow = 0.5;
            *feather = 0.75;
        }

        Sidebar::apply_mask_properties_action(
            &mut mask,
            0,
            super::super::super::adjustment_cards::CardAction::Reset,
        );

        match &mask.components[0].geometry {
            MaskGeometry::Path {
                points,
                grow,
                feather,
            } => {
                assert_eq!(points, &saved);
                assert_eq!(*grow, 0.0);
                assert_eq!(*feather, 0.0);
            }
            _ => panic!("path selection changed type"),
        }
    }
}
