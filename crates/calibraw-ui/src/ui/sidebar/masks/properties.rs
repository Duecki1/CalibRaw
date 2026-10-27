use super::*;
use crate::app::{MaskPropertiesControls, MaskPropertyAction, PeopleChoices, PersonThumbnail};
use crate::pipeline::PersonPart;
use crate::pipeline::DEFAULT_COLOR_TOLERANCE;
use crate::ui::components::luminance_range_slider::{luminance_range_slider, LuminanceRange};

/// Edge of a person's chip in the properties panel, in points.
const PERSON_CHIP_SIZE: f32 = 44.0;

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
                            MaskGeometry::Person {
                                anchor,
                                parts,
                                mask,
                                ..
                            },
                            MaskGeometry::Person {
                                anchor: person,
                                parts: saved_parts,
                                mask: saved,
                                ..
                            },
                        ) => {
                            *anchor = person;
                            *parts = saved_parts;
                            *mask = saved;
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

    /// The cards of `components`, then an Add effect button. Draw them below
    /// every other card, so the button ends the sidebar and effects added
    /// from the Effects card ([`mask_effects::add_effect_controls`]) join them
    /// there.
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
        if let Some(effect) = mask_effects::add_effect_button(ui, components) {
            components.push(crate::pipeline::EffectComponent::new(effect));
            moduwu_design::request_scroll_into_view(
                ui.ctx(),
                mask_effects::effect_card_scroll_target(effect),
                None,
            );
            changed = true;
        }
        moduwu_design::card_gap(ui);
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
            MaskEffect::Sharpen => mask_effects::sharpen::show(
                ui,
                &mut component.settings.sharpen,
                &mut component.enabled,
                remove,
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
            .show(ui)
    }

    fn mask_feather_slider(
        ui: &mut Ui,
        label: &str,
        feather: &mut f32,
        range: std::ops::RangeInclusive<f32>,
        reset: f32,
    ) -> bool {
        AdjustmentSlider::new(label, feather, range)
            .decimals(2)
            .step(0.01)
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
        people: &PeopleChoices,
    ) -> Vec<MaskPropertyAction> {
        let mut edits = PropertyEdits {
            actions: Vec::new(),
            controls: *controls,
        };
        let mut opacity = mask.opacity;
        if AdjustmentSlider::new("Mask opacity", &mut opacity, 0.0..=1.0)
            .decimals(2)
            .step(0.01)
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
                        0.55,
                    ));
                }
                MaskGeometry::Linear { feather, .. } => {
                    edits.feather(Self::mask_feather_edit(
                        ui,
                        "Feather",
                        *feather,
                        0.02..=1.0,
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
                    if generated_mask.is_none() && edits.controls.generation_idle {
                        Self::generate_mask_row(ui, is_sky, &mut edits);
                    }
                    edits.grow(Self::mask_grow_edit(ui, *grow));
                    edits.feather(Self::mask_feather_edit(
                        ui,
                        "Feather",
                        *feather,
                        0.0..=1.0,
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
                    sampled,
                    ..
                } => Self::color_range_properties(ui, *sampled, *tolerance, *grow, &mut edits),
                MaskGeometry::DepthRange { depth, range } => {
                    Self::depth_range_properties(ui, depth.is_some(), *range, &mut edits);
                }
                MaskGeometry::Person { grow, feather, .. } => {
                    Self::person_properties(ui, people, *grow, *feather, &mut edits);
                }
                MaskGeometry::Placeholder => {
                    ui.label("This mask has no shape data.");
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
            let mut invert = component.invert;
            if moduwu_design::toggle(ui, &mut invert, "Invert").changed() {
                edits.push(MaskPropertyAction::ToggleInvert);
            }
            if component_index > 0 {
                // A combo box lays out in a child `Ui` that never wraps, so
                // start a new row when it would not fit.
                if ui.available_size_before_wrap().x < ui.spacing().combo_width {
                    ui.end_row();
                }
                let mut combine = component.combine;
                egui::ComboBox::from_id_salt("vertical-mask-combine")
                    .selected_text(combine.label())
                    // Flat rows, so their labels do not shift on hover.
                    .popup_style(egui::style::StyleModifier::new(
                        egui::containers::menu::menu_style,
                    ))
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
            .reset_to(0.055)
            .show(ui)
        {
            edits.push(MaskPropertyAction::SetBrushSize(size));
        }
        if let Some(feather) = Self::mask_feather_edit(ui, "Feather", *feather, 0.0..=1.0, 0.55) {
            edits.push(MaskPropertyAction::SetBrushFeather(feather));
        }
        // Toggling opacity enables the stroke opacity slider in the same frame.
        let mut opacity_enabled = *opacity_enabled;
        if moduwu_design::toggle_with_help(
            ui,
            &mut opacity_enabled,
            "Opacity",
            "Use the opacity setting for newly drawn brush and eraser strokes. \
             Disabled strokes always use 100% opacity.",
        )
        .changed()
        {
            edits.push(MaskPropertyAction::ToggleBrushOpacity);
        }
        let mut overlap = *overlap_enabled;
        if moduwu_design::toggle_with_help(
            ui,
            &mut overlap,
            "Overlapping",
            "Allow separate brush strokes to build opacity where they overlap. \
             For example, 10% over 10% produces about 19% coverage.",
        )
        .changed()
        {
            edits.push(MaskPropertyAction::ToggleBrushOverlap);
        }
        ui.add_enabled_ui(opacity_enabled, |ui| {
            let mut opacity = *opacity;
            if AdjustmentSlider::new("Stroke opacity", &mut opacity, 0.0..=1.0)
                .decimals(2)
                .step(0.01)
                .reset_to(1.0)
                .show(ui)
            {
                edits.push(MaskPropertyAction::SetBrushStrokeOpacity(opacity));
            }
        });
        if moduwu_design::ghost_icon_button(
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
            0.0,
        ));
        let mut edge_refine = *edge_refine;
        if AdjustmentSlider::new("Edge refine", &mut edge_refine, 0.0..=1.0)
            .decimals(2)
            .step(0.01)
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
            if moduwu_design::ghost_icon_button(
                ui,
                egui_phosphor::regular::ARROW_CLOCKWISE,
                moduwu_design::toolbar_icon_size(),
                "Recalculate object selection",
            )
            .clicked()
            {
                edits.push(MaskPropertyAction::RequestObject);
            }
            if moduwu_design::ghost_icon_button(
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
            .reset_to(DEFAULT_COLOR_TOLERANCE)
            .show(ui)
        {
            edits.push(MaskPropertyAction::SetColorTolerance(tolerance));
        }
        edits.grow(Self::mask_grow_edit(ui, grow));
    }

    fn depth_range_properties(
        ui: &mut Ui,
        has_depth: bool,
        range: crate::pipeline::DepthRangeSettings,
        edits: &mut PropertyEdits,
    ) {
        ui.weak("Experimental");
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

    /// A chip per person found when there is a choice, the parts to select
    /// and the shape sliders. Until the people are known, a button finds
    /// them.
    fn person_properties(
        ui: &mut Ui,
        people: &PeopleChoices,
        grow: f32,
        feather: f32,
        edits: &mut PropertyEdits,
    ) {
        if !people.analysed {
            let clicked = ui
                .with_layout(
                    egui::Layout::top_down_justified(egui::Align::Center),
                    |ui| moduwu_design::secondary_button(ui, "Find people").clicked(),
                )
                .inner;
            if clicked {
                edits.push(MaskPropertyAction::RequestPeople);
            }
            ui.add_space(moduwu_design::SPACE_XS);
        } else if people.people.len() > 1 {
            ui.horizontal_wrapped(|ui| {
                for (index, person) in people.people.iter().enumerate() {
                    let selected = people.selected == Some(index);
                    let response = person_chip(ui, person, selected, index);
                    if response.hovered() {
                        edits.push(MaskPropertyAction::HoverPerson(index));
                    }
                    if response.clicked() && !selected {
                        edits.push(MaskPropertyAction::SelectPerson(person.anchor));
                    }
                }
            });
            ui.add_space(moduwu_design::SPACE_XS);
        }
        ui.horizontal_wrapped(|ui| {
            for part in PersonPart::ALL {
                let selected = people.parts.contains(&part);
                // A selected part stays enabled so it can be removed.
                let shown = selected
                    || people
                        .shown
                        .as_ref()
                        .is_none_or(|shown| shown.contains(&part));
                // `add_enabled` lays the chip out in the row itself; a child
                // `Ui` would never wrap to the next row.
                let clicked = ui
                    .add_enabled(shown, |ui: &mut Ui| {
                        moduwu_design::chip(ui, part.label(), selected)
                    })
                    .clicked();
                if clicked {
                    edits.push(MaskPropertyAction::TogglePersonPart(part));
                }
            }
        });
        if people.parts.len() > 1
            && moduwu_design::secondary_button(ui, "Separate masks")
                .on_hover_text("Move each part to a mask group of its own")
                .clicked()
        {
            edits.push(MaskPropertyAction::SeparatePersonParts);
        }
        ui.add_space(moduwu_design::SPACE_XS);
        edits.grow(Self::mask_grow_edit(ui, grow));
        edits.feather(Self::mask_feather_edit(
            ui,
            "Feather",
            feather,
            0.0..=1.0,
            0.0,
        ));
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
        reset: f32,
    ) -> Option<f32> {
        let mut feather = feather;
        Self::mask_feather_slider(ui, label, &mut feather, range, reset).then_some(feather)
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
                .reset_to(0.035)
                .show(ui)
            {
                edits.set_refinement_size(size);
            }
            let mut feather = edits.controls.refinement_feather;
            if Self::mask_feather_slider(ui, "Feather", &mut feather, 0.0..=1.0, 0.55) {
                edits.set_refinement_feather(feather);
            }
            let mut flow = edits.controls.refinement_flow;
            if AdjustmentSlider::new("Flow / opacity", &mut flow, 0.01..=1.0)
                .decimals(2)
                .step(0.01)
                .reset_to(1.0)
                .show(ui)
            {
                edits.set_refinement_flow(flow);
            }
            if moduwu_design::ghost_icon_button(
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
        let (label, model) = if is_sky {
            ("Generate sky mask", "SkySeg U2Net")
        } else {
            ("Generate subject mask", "BiRefNet General-Lite")
        };
        let clicked = ui
            .with_layout(
                egui::Layout::top_down_justified(egui::Align::Center),
                |ui| {
                    moduwu_design::secondary_button(ui, label)
                        .on_hover_text(format!("Runs {model} locally."))
                        .clicked()
                },
            )
            .inner;
        if clicked {
            edits.push(MaskPropertyAction::RequestGeneration);
        }
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

/// A person's head as a selectable chip, announced as "Person n". Someone
/// not segmented yet is dimmed: choosing them runs the body-part model.
fn person_chip(
    ui: &mut Ui,
    person: &PersonThumbnail,
    selected: bool,
    index: usize,
) -> egui::Response {
    let tint = if person.segmented {
        egui::Color32::WHITE
    } else {
        egui::Color32::WHITE.gamma_multiply(0.45)
    };
    let response = ui.add(
        egui::Button::image(
            egui::Image::new((person.texture, egui::Vec2::splat(PERSON_CHIP_SIZE)))
                .corner_radius(4.0)
                .tint(tint),
        )
        .selected(selected)
        .corner_radius(6.0),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Button,
            ui.is_enabled(),
            selected,
            format!("Person {}", index + 1),
        )
    });
    response
}
