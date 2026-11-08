use super::*;
use crate::app::{MaskPropertiesControls, MaskPropertyAction, PeopleChoices, PersonThumbnail};
use crate::i18n::{tr, tr_fmt};
use crate::pipeline::PersonPart;
use crate::pipeline::DEFAULT_COLOR_TOLERANCE;
use crate::ui::components::luminance_range_slider::{luminance_range_slider, LuminanceRange};
use std::collections::BTreeSet;

/// Edge of a person's chip in the properties panel, in points.
const PERSON_CHIP_SIZE: f32 = 44.0;

impl Sidebar {
    /// Reset the group's switches and opacity. Returns whether anything reset.
    pub(super) fn reset_mask_group(
        mask: &mut LocalMask,
        action: super::super::adjustment_cards::CardAction,
    ) -> bool {
        use super::super::adjustment_cards::CardAction;
        match action {
            CardAction::None | CardAction::Toggle => false,
            CardAction::Reset => {
                mask.enabled = true;
                mask.invert = false;
                mask.opacity = 1.0;
                true
            }
        }
    }

    /// Reset a sub-mask's switches, opacity and property controls, keeping
    /// its selection itself. Returns whether anything reset.
    pub(super) fn reset_submask(
        mask: &mut LocalMask,
        component_index: usize,
        action: super::super::adjustment_cards::CardAction,
    ) -> bool {
        use super::super::adjustment_cards::CardAction;
        match action {
            CardAction::None => return false,
            CardAction::Toggle => {}
            CardAction::Reset => {
                if let Some(component) = mask.components.get_mut(component_index) {
                    component.enabled = true;
                    component.invert = false;
                    component.opacity = 1.0;
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
                                anchors,
                                parts,
                                mask,
                                ..
                            },
                            MaskGeometry::Person {
                                anchors: people,
                                parts: saved_parts,
                                mask: saved,
                                ..
                            },
                        ) => {
                            *anchors = people;
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
    /// there. Returns whether a setting changed and whether the button's
    /// `film_negative` tile was picked.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::ui::sidebar) fn show_effect_components(
        ui: &mut Ui,
        components: &mut Vec<crate::pipeline::EffectComponent>,
        list: crate::app::EffectList,
        instances: &mut crate::app::SelectedInstances,
        is_fullscreen_mask: bool,
        film_negative: mask_effects::FilmNegativeTile,
        frame: &mask_effects::EffectFrame,
    ) -> (bool, bool) {
        let mut changed = false;
        let mut remove = None;
        let room = Self::effect_list_has_room(components);
        for (index, component) in components.iter_mut().enumerate() {
            ui.push_id(index, |ui| {
                let mut remove_component = false;
                changed |= Self::show_effect_component_settings(
                    ui,
                    component,
                    &mut remove_component,
                    is_fullscreen_mask,
                    frame,
                    mask_effects::InstanceSelection {
                        selected: instances.entry(list, component.effect),
                        room,
                    },
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
        let picked = mask_effects::add_effect_button(ui, components, film_negative);
        if let Some(effect) = picked.and_then(mask_effects::EffectPick::effect) {
            components.push(crate::pipeline::EffectComponent::new(effect));
            mask_effects::reveal_effect_card(ui.ctx(), effect);
            changed = true;
        }
        moduwu_design::card_gap(ui);
        (
            changed,
            picked == Some(mask_effects::EffectPick::FilmNegative),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn show_selected_effect_component(
        ui: &mut Ui,
        components: &mut Vec<crate::pipeline::EffectComponent>,
        list: crate::app::EffectList,
        instances: &mut crate::app::SelectedInstances,
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
        let room = Self::effect_list_has_room(components);
        let changed = ui
            .push_id(index, |ui| {
                Self::show_effect_component_settings(
                    ui,
                    &mut components[index],
                    &mut remove,
                    is_fullscreen_mask,
                    frame,
                    mask_effects::InstanceSelection {
                        selected: instances.entry(list, effect),
                        room,
                    },
                )
            })
            .inner;
        if remove {
            components.remove(index);
            *selection = None;
        }
        changed || remove
    }

    /// Whether an effect list has a place for one more effect or instance
    /// ([`crate::pipeline::effect_places`]).
    fn effect_list_has_room(components: &[crate::pipeline::EffectComponent]) -> bool {
        crate::pipeline::effect_places(components) < crate::pipeline::MAX_EFFECT_COMPONENTS
    }

    /// `instances` is the light or source the card shows, for effects with
    /// several.
    pub(super) fn show_effect_component_settings(
        ui: &mut Ui,
        component: &mut crate::pipeline::EffectComponent,
        remove: &mut bool,
        is_fullscreen_mask: bool,
        frame: &mask_effects::EffectFrame,
        instances: mask_effects::InstanceSelection<'_>,
    ) -> bool {
        // Effects with a position pad, a range track or a conditional
        // control draw their own card; every other effect's card is drawn
        // from its controls.
        match component.effect {
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
            MaskEffect::LightRays => mask_effects::light_rays::show(
                ui,
                &mut component.settings.light_rays,
                &mut component.enabled,
                remove,
                frame,
                instances,
            ),
            MaskEffect::Relight => mask_effects::relight::show(
                ui,
                &mut component.settings.relight,
                &mut component.enabled,
                remove,
                frame,
                instances,
            ),
            MaskEffect::Fog => mask_effects::fog::show(
                ui,
                &mut component.settings.fog,
                &mut component.enabled,
                remove,
            ),
            MaskEffect::Rain => mask_effects::rain::show(
                ui,
                &mut component.settings.rain,
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
            effect => mask_effects::generated_card(
                ui,
                effect,
                &mut component.settings,
                &mut component.enabled,
                remove,
            ),
        }
    }

    pub(super) fn is_plain_fullscreen_mask(mask: &LocalMask) -> bool {
        !mask.invert
            && matches!(
                mask.components.as_slice(),
                [component]
                    if component.enabled
                        && !component.invert
                        && component.opacity == 1.0
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
        AdjustmentSlider::new(tr("Grow"), grow, -1.0..=1.0)
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

    /// The group card's body: the group's name and switches, and its
    /// opacity. Returns the edits in the order they apply.
    pub(super) fn show_mask_group_properties(
        ui: &mut Ui,
        mask: &LocalMask,
    ) -> Vec<MaskPropertyAction> {
        let mut actions = Vec::new();
        let switches = switches_row(ui, mask.name.as_str(), mask.enabled, mask.invert);
        if switches.enabled {
            actions.push(MaskPropertyAction::SetGroupEnabled(!mask.enabled));
        }
        if switches.invert {
            actions.push(MaskPropertyAction::ToggleGroupInvert);
        }
        let mut opacity = mask.opacity;
        if AdjustmentSlider::new(tr("Mask opacity"), &mut opacity, 0.0..=1.0)
            .decimals(2)
            .step(0.01)
            .reset_to(1.0)
            .show(ui)
        {
            actions.push(MaskPropertyAction::SetOpacity(opacity));
        }
        actions
    }

    /// The sub-mask card's body: the selected sub-mask's name, switches,
    /// combine mode and opacity, then its kind's properties. Returns the
    /// edits, in the order they apply; `controls` is the tool state at the
    /// start of the frame.
    pub(super) fn show_submask_properties(
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
        let Some(component) = mask.components.get(component_index) else {
            return edits.actions;
        };

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
                        tr("Feather"),
                        *feather,
                        0.0..=1.0,
                        0.55,
                    ));
                }
                MaskGeometry::Linear { feather, .. } => {
                    edits.feather(Self::mask_feather_edit(
                        ui,
                        tr("Feather"),
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
                        tr("Feather"),
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
                    ui.label(tr("This mask has no shape data."));
                }
            }
        });

        edits.actions
    }

    /// The component's name and switches, after the base its combine mode,
    /// and its opacity.
    fn component_header(
        ui: &mut Ui,
        component: &MaskComponent,
        component_index: usize,
        edits: &mut PropertyEdits,
    ) {
        let switches = switches_row(
            ui,
            component.name.as_str(),
            component.enabled,
            component.invert,
        );
        if matches!(&component.geometry, MaskGeometry::Fullscreen) {
            switches
                .name
                .on_hover_text(tr("Covers the complete image with uniform mask strength."));
        }
        if switches.enabled {
            edits.push(MaskPropertyAction::SetComponentEnabled(!component.enabled));
        }
        if switches.invert {
            edits.push(MaskPropertyAction::ToggleComponentInvert);
        }
        if component_index > 0 {
            let mut combine = component.combine;
            egui::ComboBox::from_id_salt("vertical-mask-combine")
                .selected_text(tr(combine.label()))
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
                        ui.selectable_value(&mut combine, mode, tr(mode.label()));
                    }
                });
            if combine != component.combine {
                edits.push(MaskPropertyAction::SetCombine(combine));
            }
        }
        let mut opacity = component.opacity;
        if AdjustmentSlider::new(tr("Sub-mask opacity"), &mut opacity, 0.0..=1.0)
            .decimals(2)
            .step(0.01)
            .reset_to(1.0)
            .show(ui)
        {
            edits.push(MaskPropertyAction::SetComponentOpacity(opacity));
        }
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
        Self::brush_mode_selector(ui, edits, tr("Brush"), tr("Eraser"));
        let mut size = *size;
        if AdjustmentSlider::new(tr("Size"), &mut size, 0.0025..=0.25)
            .decimals(3)
            .step(0.0025)
            .reset_to(0.055)
            .show(ui)
        {
            edits.push(MaskPropertyAction::SetBrushSize(size));
        }
        if let Some(feather) = Self::mask_feather_edit(ui, tr("Feather"), *feather, 0.0..=1.0, 0.55)
        {
            edits.push(MaskPropertyAction::SetBrushFeather(feather));
        }
        // Toggling opacity enables the stroke opacity slider in the same frame.
        let mut opacity_enabled = *opacity_enabled;
        if moduwu_design::toggle_with_help(
            ui,
            &mut opacity_enabled,
            tr("Opacity"),
            tr("Use the opacity setting for newly drawn brush and eraser strokes. Disabled strokes always use 100% opacity."),
        )
        .changed()
        {
            edits.push(MaskPropertyAction::ToggleBrushOpacity);
        }
        let mut overlap = *overlap_enabled;
        if moduwu_design::toggle_with_help(
            ui,
            &mut overlap,
            tr("Overlapping"),
            tr("Allow separate brush strokes to build opacity where they overlap. For example, 10% over 10% produces about 19% coverage."),
        )
        .changed()
        {
            edits.push(MaskPropertyAction::ToggleBrushOverlap);
        }
        ui.add_enabled_ui(opacity_enabled, |ui| {
            let mut opacity = *opacity;
            if AdjustmentSlider::new(tr("Stroke opacity"), &mut opacity, 0.0..=1.0)
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
            tr("Clear brush strokes"),
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
        ui.label(tr("Click to add polygon points. Click-drag while adding a point to create a smooth Bézier point. Drag anchors or handles to edit the path; Alt/Option-drag an anchor to create symmetric handles."));
        edits.grow(Self::mask_grow_edit(ui, grow));
        edits.feather(Self::mask_feather_edit(
            ui,
            tr("Feather"),
            feather,
            0.0..=1.0,
            0.0,
        ));
        // The count below reflects this frame's edits.
        let mut point_count = point_count;
        ui.horizontal_wrapped(|ui| {
            if ui
                .small_button(tr("Straighten"))
                .on_hover_text(tr("Convert all path points to straight corners"))
                .clicked()
            {
                edits.push(MaskPropertyAction::StraightenPath);
            }
            if ui
                .small_button(tr("Undo point"))
                .on_hover_text(tr("Remove the last path point"))
                .clicked()
            {
                edits.push(MaskPropertyAction::RemoveLastPathPoint);
                point_count = point_count.saturating_sub(1);
            }
            if ui
                .small_button(tr("Clear"))
                .on_hover_text(tr("Clear path"))
                .clicked()
                && point_count > 0
            {
                edits.push(MaskPropertyAction::ClearPath);
                point_count = 0;
            }
        });
        ui.small(tr_fmt("{count} point(s)", &[("count", &point_count)]));
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
            tr("Draw again on the image to replace this object selection from scratch.")
        } else {
            tr("Paint through the middle of the object part you want to select.")
        });
        ui.strong(tr("Selection brush"));
        let mut brush_size = *brush_size;
        if AdjustmentSlider::new(tr("Size"), &mut brush_size, 0.0025..=0.25)
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
            tr("Mask feather"),
            *feather,
            0.0..=1.0,
            0.0,
        ));
        let mut edge_refine = *edge_refine;
        if AdjustmentSlider::new(tr("Edge refine"), &mut edge_refine, 0.0..=1.0)
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
                tr("Recalculate object selection"),
            )
            .clicked()
            {
                edits.push(MaskPropertyAction::RequestObject);
            }
            if moduwu_design::ghost_icon_button(
                ui,
                egui_phosphor::regular::X,
                moduwu_design::toolbar_icon_size(),
                tr("Clear object selection"),
            )
            .clicked()
            {
                edits.push(MaskPropertyAction::ClearObjectSelection);
                stroke_count = 0;
            }
        });
        ui.small(tr_fmt(
            "{count} selection stroke(s)",
            &[("count", &stroke_count)],
        ));
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
            tr("Drag on the image to choose another color.")
        } else {
            tr("Drag on the image to sample a color.")
        });
        let mut tolerance = tolerance;
        if AdjustmentSlider::new(tr("Tolerance"), &mut tolerance, 0.005..=1.0)
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
        ui.weak(tr("Experimental"));
        ui.label(if has_depth {
            tr("Select a range of relative depth, from near (0) to far (1).")
        } else {
            tr("Generate a depth map to select by distance.")
        });
        let label = if has_depth {
            tr("Regenerate depth map")
        } else {
            tr("Generate depth map")
        };
        if moduwu_design::secondary_button(ui, label).clicked() {
            edits.push(MaskPropertyAction::RequestGeneration);
        }
        let mut range = range;
        if crate::ui::components::depth_range_slider::depth_range_slider(ui, &mut range) {
            edits.push(MaskPropertyAction::SetDepthRange(range));
        }
    }

    /// A chip per person found when there is a choice, which adds them or
    /// removes them, a switch per part to select and the shape sliders.
    /// Until the people are known, a button finds them.
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
                    |ui| moduwu_design::secondary_button(ui, tr("Find people")).clicked(),
                )
                .inner;
            if clicked {
                edits.push(MaskPropertyAction::RequestPeople);
            }
            ui.add_space(moduwu_design::SPACE_XS);
        } else if people.people.len() > 1 {
            ui.horizontal_wrapped(|ui| {
                for (index, person) in people.people.iter().enumerate() {
                    let selected = people.selected.contains(&index);
                    let response = person_chip(ui, person, selected, index);
                    if response.hovered() {
                        edits.push(MaskPropertyAction::HoverPerson(index));
                    }
                    if response.clicked() {
                        edits.push(MaskPropertyAction::TogglePerson(index));
                    }
                }
            });
            ui.add_space(moduwu_design::SPACE_XS);
        }
        // Where the entire person is all a device can select, there is
        // nothing to choose.
        let entire_only = |parts: &BTreeSet<PersonPart>| {
            parts.iter().all(|&part| part == PersonPart::EntirePerson)
        };
        let choosable = !entire_only(&people.parts)
            || people
                .shown
                .as_ref()
                .is_none_or(|shown| !entire_only(shown));
        for part in PersonPart::ALL.into_iter().filter(|_| choosable) {
            let mut selected = people.parts.contains(&part);
            // A selected part stays enabled so it can be removed.
            let shown = selected
                || people
                    .shown
                    .as_ref()
                    .is_none_or(|shown| shown.contains(&part));
            let changed = ui
                .add_enabled(shown, |ui: &mut Ui| {
                    moduwu_design::toggle_row(ui, &mut selected, tr(part.label()))
                })
                .changed();
            if changed {
                edits.push(MaskPropertyAction::TogglePersonPart(part));
            }
        }
        if people.parts.len() > 1
            && moduwu_design::secondary_button(ui, tr("Separate masks"))
                .on_hover_text(tr("Move each part to a mask group of its own"))
                .clicked()
        {
            edits.push(MaskPropertyAction::SeparatePersonParts);
        }
        ui.add_space(moduwu_design::SPACE_XS);
        edits.grow(Self::mask_grow_edit(ui, grow));
        edits.feather(Self::mask_feather_edit(
            ui,
            tr("Feather"),
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
        let toggle_label = if active { tr("Done") } else { tr("Refine") };
        if moduwu_design::toggle_button(ui, toggle_label, active)
            .on_hover_text(tr(
                "Fine-tune the shared Subject / Background boundary with a brush.",
            ))
            .clicked()
        {
            edits.controls.refinement_active = !active;
            edits.push(MaskPropertyAction::SetRefinementActive(!active));
        }
        if !edits.controls.refinement_active {
            return;
        }
        let action = Self::adjustment_card(ui, "Subject refinement", true, false, true, |ui| {
            Self::brush_mode_selector(ui, edits, tr("Add subject"), tr("Subtract subject"));
            let mut size = edits.controls.refinement_size;
            if AdjustmentSlider::new(tr("Size"), &mut size, 0.0025..=0.25)
                .decimals(3)
                .step(0.0025)
                .reset_to(0.035)
                .show(ui)
            {
                edits.set_refinement_size(size);
            }
            let mut feather = edits.controls.refinement_feather;
            if Self::mask_feather_slider(ui, tr("Feather"), &mut feather, 0.0..=1.0, 0.55) {
                edits.set_refinement_feather(feather);
            }
            let mut flow = edits.controls.refinement_flow;
            if AdjustmentSlider::new(tr("Flow / opacity"), &mut flow, 0.01..=1.0)
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
                tr("Clear subject refinement"),
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
            (tr("Generate sky mask"), "SkySeg U2Net")
        } else {
            (tr("Generate subject mask"), "BiRefNet General-Lite")
        };
        let clicked = ui
            .with_layout(
                egui::Layout::top_down_justified(egui::Align::Center),
                |ui| {
                    moduwu_design::secondary_button(ui, label)
                        .on_hover_text(tr_fmt("Runs {model} locally.", &[("model", &model)]))
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

/// A person's head as a chip that adds them to the mask or removes them,
/// announced as a "Person n" checkbox. Someone not segmented yet is dimmed:
/// adding them runs the body-part model.
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
            egui::WidgetType::Checkbox,
            ui.is_enabled(),
            selected,
            tr_fmt("Person {number}", &[("number", &(index + 1))]),
        )
    });
    response
}

/// Which of a [`switches_row`]'s switches were clicked, and its name label.
struct Switches {
    name: egui::Response,
    enabled: bool,
    invert: bool,
}

/// A group's or sub-mask's name with its Enabled and Invert switches at the
/// row's end, as chips that fill while on. Enabled shows a power symbol, as
/// the card header's eye already toggles the mask overlay.
fn switches_row(ui: &mut Ui, name: &str, enabled: bool, invert: bool) -> Switches {
    use egui_phosphor::regular;

    let height = ui.spacing().interact_size.y;
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width().max(1.0), height),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            let invert = moduwu_design::chip(
                ui,
                format!("{}  {}", regular::SELECTION_INVERSE, tr("Invert")),
                invert,
            )
            .clicked();
            let enabled = moduwu_design::chip(
                ui,
                format!("{}  {}", regular::POWER, tr("Enabled")),
                enabled,
            )
            .clicked();
            let name = ui
                .with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(name).strong()).truncate())
                })
                .inner;
            Switches {
                name,
                enabled,
                invert,
            }
        },
    )
    .inner
}
