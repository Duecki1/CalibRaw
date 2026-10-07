use super::*;
use crate::pipeline::{effect_params as params, EffectComponent, MaskEffectSettings};
use crate::ui::sidebar::Sidebar;

fn text_rect(shapes: &[egui::epaint::ClippedShape], label: &str) -> Option<egui::Rect> {
    fn find(shape: &egui::Shape, label: &str) -> Option<egui::Rect> {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == label => {
                Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, label)),
            _ => None,
        }
    }
    shapes.iter().find_map(|shape| find(&shape.shape, label))
}

fn pointer(position: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(position),
        egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        },
    ]
}

struct CardUi {
    ctx: egui::Context,
    components: Vec<EffectComponent>,
    selection: Option<MaskEffect>,
    viewport: egui::Vec2,
    width: f32,
    time: f64,
    shapes: Vec<egui::epaint::ClippedShape>,
    used: egui::Rect,
}

impl CardUi {
    fn new(effect: MaskEffect, width: f32, portrait: bool) -> Self {
        let ctx = egui::Context::default();
        crate::ui::theme::install(&ctx);
        ctx.style_mut_of(egui::Theme::Dark, |style| style.animation_time = 0.0);
        Self {
            ctx,
            components: vec![EffectComponent::new(effect)],
            selection: Some(effect),
            viewport: if portrait {
                egui::vec2(width, 1600.0)
            } else {
                egui::vec2(1800.0, 1600.0)
            },
            width,
            time: 0.0,
            shapes: Vec::new(),
            used: egui::Rect::NOTHING,
        }
    }

    fn frame(&mut self, events: Vec<egui::Event>) -> bool {
        self.time += 0.25;
        let mut changed = false;
        let output = self.ctx.run_ui(
            egui::RawInput {
                time: Some(self.time),
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.viewport)),
                events,
                ..Default::default()
            },
            |ui| {
                ui.set_width(self.width);
                Sidebar::begin_vertical_card_actions(ui.ctx());
                self.used = ui
                    .vertical(|ui| {
                        changed = Sidebar::show_selected_effect_component(
                            ui,
                            &mut self.components,
                            &mut self.selection,
                            true,
                            &EffectFrame::uncropped(3000, 2000),
                        );
                        if self.viewport.y > self.viewport.x {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    Sidebar::show_vertical_card_footer_actions(ui);
                                },
                            );
                        }
                    })
                    .response
                    .rect;
            },
        );
        self.shapes = output.shapes;
        changed
    }

    fn has(&self, text: &str) -> bool {
        text_rect(&self.shapes, text).is_some()
    }

    fn click(&mut self, text: &str) -> bool {
        let target = text_rect(&self.shapes, text)
            .unwrap_or_else(|| panic!("missing control {text}"))
            .center();
        let mut changed = self.frame(pointer(target, true));
        changed |= self.frame(pointer(target, false));
        // Portrait actions are consumed by the card on the next frame.
        changed |= self.frame(Vec::new());
        changed
    }
}

#[test]
fn primary_controls_are_visible_and_details_expand_without_editing() {
    let cases: &[(MaskEffect, &[&str], &str, &[&str])] = &[
        (
            MaskEffect::LensBlur,
            &[
                params::lens_blur::AMOUNT.label,
                params::lens_blur::RADIUS.label,
                params::lens_blur::HIGHLIGHTS.label,
            ],
            "Aperture details",
            &[
                params::lens_blur::BLADES.label,
                params::lens_blur::ROTATION.label,
            ],
        ),
        (
            MaskEffect::RadialBlur,
            &[
                "Zoom",
                "Spin",
                params::radial_blur::STRENGTH.label,
                "Blur center",
            ],
            "Precise position",
            &[
                params::radial_blur::CENTER_X.label,
                params::radial_blur::CENTER_Y.label,
            ],
        ),
        (
            MaskEffect::TiltShift,
            &[
                params::tilt_shift::AMOUNT.label,
                params::tilt_shift::RADIUS.label,
                params::tilt_shift::FOCUS_WIDTH.label,
            ],
            "Focus band details",
            &[
                params::tilt_shift::ANGLE.label,
                params::tilt_shift::FEATHER.label,
            ],
        ),
        (
            MaskEffect::Glow,
            &[
                params::glow::AMOUNT.label,
                params::glow::RADIUS.label,
                params::glow::COLOR.label,
            ],
            "Light details",
            &[params::glow::CORE.label],
        ),
        (
            MaskEffect::EdgeGlow,
            &[
                params::edge_glow::AMOUNT.label,
                params::edge_glow::EDGE_WIDTH.label,
            ],
            "Edge details",
            &[
                params::edge_glow::DETAIL.label,
                params::edge_glow::GLOW.label,
            ],
        ),
        (
            MaskEffect::Neon,
            &[params::neon::AMOUNT.label, params::neon::EDGE_WIDTH.label],
            "Neon details",
            &[params::neon::DETAIL.label, params::neon::BACKGROUND.label],
        ),
        (
            MaskEffect::LightRays,
            &[
                params::light_rays::AMOUNT.label,
                params::light_rays::LENGTH.label,
                "Source position",
            ],
            "Ray details",
            &[
                params::light_rays::SPREAD.label,
                params::light_rays::RAY_COUNT.label,
            ],
        ),
        (
            MaskEffect::Relight,
            &[
                params::relight::AMOUNT.label,
                "Light position",
                params::relight::DEPTH.label,
                params::relight::AMBIENT.label,
                "Cast shadows",
            ],
            "Light details",
            &[
                params::relight::REACH.label,
                params::relight::SIZE.label,
                params::relight::SHADOWS.label,
                params::relight::RELIEF.label,
            ],
        ),
        (
            MaskEffect::Fog,
            &[
                params::fog::AMOUNT.label,
                params::fog::DENSITY.label,
                "Scene depth",
            ],
            "Atmosphere details",
            &[
                params::fog::DEPTH_INFLUENCE.label,
                params::fog::VARIATION.label,
            ],
        ),
        (
            MaskEffect::Smoke,
            &[
                params::smoke::AMOUNT.label,
                params::smoke::DENSITY.label,
                params::smoke::SCALE.label,
            ],
            "Texture details",
            &[params::smoke::TURBULENCE.label, params::smoke::SEED.label],
        ),
        (
            MaskEffect::Grain,
            &[params::grain::AMOUNT.label, params::grain::SIZE.label],
            "Texture details",
            &[
                params::grain::ROUGHNESS.label,
                params::grain::COLOR.label,
                params::grain::SEED.label,
            ],
        ),
        (
            MaskEffect::Halation,
            &[
                params::halation::AMOUNT.label,
                params::halation::RADIUS.label,
                params::halation::THRESHOLD.label,
            ],
            "Color details",
            &[params::halation::WARMTH.label],
        ),
        (
            MaskEffect::Vignette,
            &[
                params::vignette::AMOUNT.label,
                params::vignette::MIDPOINT.label,
                params::vignette::FEATHER.label,
            ],
            "Shape details",
            &[params::vignette::ROUNDNESS.label, "Vignette center"],
        ),
    ];
    for portrait in [false, true] {
        for &(effect, primary, section, detailed) in cases {
            let mut card = CardUi::new(effect, 320.0, portrait);
            let before = card.components.clone();
            assert!(!card.frame(Vec::new()));
            for label in primary {
                assert!(card.has(label), "{effect:?}: missing primary {label}");
            }
            for label in detailed {
                assert!(
                    !card.has(label),
                    "{effect:?}: detail {label} shown initially"
                );
            }
            assert!(!card.click(section));
            for label in detailed {
                assert!(
                    card.has(label),
                    "{effect:?}: missing expanded detail {label}"
                );
            }
            assert_eq!(
                card.components, before,
                "opening {effect:?} details edits settings"
            );
            assert!(!card.click(section));
            for label in detailed {
                assert!(
                    !card.has(label),
                    "{effect:?}: detail {label} did not collapse"
                );
            }
        }
    }
}

#[test]
fn every_effect_card_fits_narrow_and_wide_layouts() {
    for portrait in [false, true] {
        for width in [220.0, 280.0, 360.0, 480.0] {
            for effect in MaskEffect::ALL
                .into_iter()
                .filter(|effect| *effect != MaskEffect::Adjustment)
            {
                let mut card = CardUi::new(effect, width, portrait);
                card.frame(Vec::new());
                assert!(
                    card.used.width() <= width + 1.0,
                    "{effect:?}, width {width}, portrait {portrait}: {:?}",
                    card.used
                );
                for details in [
                    "Aperture details",
                    "Focus band details",
                    "Light details",
                    "Edge details",
                    "Neon details",
                    "Ray details",
                    "Atmosphere details",
                    "Texture details",
                    "Color details",
                    "Shape details",
                    "Precise position",
                ] {
                    if card.has(details) {
                        card.click(details);
                        assert!(
                            card.used.width() <= width + 1.0,
                            "{effect:?}, {details}, width {width}, portrait {portrait}: {:?}",
                            card.used
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn relight_shadow_toggle_hides_the_strength_and_preserves_it() {
    let mut card = CardUi::new(MaskEffect::Relight, 320.0, true);
    card.components[0].settings.relight.shadows = 35.0;
    card.frame(Vec::new());
    assert!(!card.click("Light details"));
    assert!(card.has(params::relight::SHADOWS.label));
    assert!(card.click("Cast shadows"));
    assert!(!card.components[0].settings.relight.shadows_enabled);
    assert!(!card.has(params::relight::SHADOWS.label));
    assert!(card.has(params::relight::SIZE.label));
    assert!(card.click("Cast shadows"));
    assert!(card.components[0].settings.relight.shadows_enabled);
    assert_eq!(card.components[0].settings.relight.shadows, 35.0);
    assert!(card.has(params::relight::SHADOWS.label));
}

#[test]
fn fog_depth_toggle_hides_only_depth_controls_and_preserves_values() {
    let mut card = CardUi::new(MaskEffect::Fog, 320.0, true);
    card.components[0].settings.fog.start = 35.0;
    card.components[0].settings.fog.depth_influence = 80.0;
    card.frame(Vec::new());
    assert!(card.has(params::fog::START.label));
    assert!(!card.has(params::fog::DEPTH_INFLUENCE.label));
    assert!(!card.click("Atmosphere details"));
    assert!(card.has(params::fog::DEPTH_INFLUENCE.label));
    assert!(card.click("Scene depth"));
    assert!(!card.components[0].settings.fog.depth_enabled);
    assert!(!card.has(params::fog::START.label));
    assert!(!card.has(params::fog::DEPTH_INFLUENCE.label));
    assert!(card.has(params::fog::DENSITY.label));
    assert!(card.click("Scene depth"));
    assert!(card.components[0].settings.fog.depth_enabled);
    assert_eq!(card.components[0].settings.fog.start, 35.0);
    assert_eq!(card.components[0].settings.fog.depth_influence, 80.0);
    assert!(card.has(params::fog::START.label));
    assert!(card.has(params::fog::DEPTH_INFLUENCE.label));
}

#[test]
fn inactive_texture_and_highlight_controls_are_hidden_without_resetting_them() {
    let mut fog = CardUi::new(MaskEffect::Fog, 320.0, true);
    fog.components[0].settings.fog.variation = 0.0;
    fog.components[0].settings.fog.seed = 321.0;
    fog.frame(Vec::new());
    fog.click("Atmosphere details");
    assert!(!fog.has(params::fog::SCALE.label));
    assert!(!fog.has(params::fog::SEED.label));
    fog.components[0].settings.fog.variation = 50.0;
    fog.frame(Vec::new());
    assert!(fog.has(params::fog::SCALE.label));
    assert!(fog.has(params::fog::SEED.label));
    assert_eq!(fog.components[0].settings.fog.seed, 321.0);

    let mut rays = CardUi::new(MaskEffect::LightRays, 320.0, true);
    rays.components[0].settings.light_rays.variation = 0.0;
    rays.frame(Vec::new());
    rays.click("Ray details");
    assert!(!rays.has(params::light_rays::RAY_COUNT.label));
    rays.components[0].settings.light_rays.variation = 50.0;
    rays.frame(Vec::new());
    assert!(rays.has(params::light_rays::RAY_COUNT.label));

    let mut vignette = CardUi::new(MaskEffect::Vignette, 320.0, true);
    vignette.components[0].settings.vignette.amount = 50.0;
    vignette.components[0].settings.vignette.highlights = 77.0;
    vignette.frame(Vec::new());
    vignette.click("Shape details");
    assert!(!vignette.has(params::vignette::HIGHLIGHTS.label));
    vignette.components[0].settings.vignette.amount = -50.0;
    vignette.frame(Vec::new());
    assert!(vignette.has(params::vignette::HIGHLIGHTS.label));
    assert_eq!(vignette.components[0].settings.vignette.highlights, 77.0);
}

#[test]
fn photographic_cards_dispatch_reset_toggle_and_remove_in_both_layouts() {
    for portrait in [false, true] {
        for effect in [
            MaskEffect::Grain,
            MaskEffect::Halation,
            MaskEffect::Vignette,
        ] {
            let mut card = CardUi::new(effect, 320.0, portrait);
            let settings = &mut card.components[0].settings;
            settings.blur.amount = 19.0;
            settings.grain.amount = 87.0;
            settings.grain.seed = 111.0;
            settings.halation.warmth = 13.0;
            settings.vignette.center = [20.0, 70.0];
            settings.vignette.amount = 91.0;
            let mut expected = *settings;
            match effect {
                MaskEffect::Grain => expected.grain = Default::default(),
                MaskEffect::Halation => expected.halation = Default::default(),
                MaskEffect::Vignette => expected.vignette = Default::default(),
                _ => unreachable!(),
            }
            card.frame(Vec::new());
            assert!(card.click(egui_phosphor::regular::EYE_SLASH));
            assert!(!card.components[0].enabled);
            assert!(card.click(egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE));
            assert_eq!(card.components[0].settings, expected);
            assert!(
                !card.components[0].enabled,
                "reset preserves the visibility choice"
            );
            assert!(card.click(egui_phosphor::regular::EYE));
            assert!(card.components[0].enabled);
            assert!(card.click(egui_phosphor::regular::TRASH));
            assert!(card.components.is_empty());
            assert_eq!(card.selection, None);
        }
    }
}

#[test]
fn radial_mode_buttons_edit_only_the_mode() {
    let mut card = CardUi::new(MaskEffect::RadialBlur, 280.0, true);
    let before = card.components[0].settings.radial_blur;
    card.frame(Vec::new());
    assert!(card.click("Spin"));
    assert_eq!(
        card.components[0].settings.radial_blur,
        crate::pipeline::RadialBlurEffectSettings {
            mode: crate::pipeline::RadialBlurMode::Spin,
            ..before
        }
    );
    assert!(card.click("Zoom"));
    assert_eq!(card.components[0].settings.radial_blur, before);
}

#[test]
fn hidden_effect_cards_reject_mode_and_depth_changes() {
    for portrait in [false, true] {
        for (effect, control) in [
            (MaskEffect::RadialBlur, "Spin"),
            (MaskEffect::Fog, "Scene depth"),
        ] {
            let mut card = CardUi::new(effect, 280.0, portrait);
            card.frame(Vec::new());
            assert!(card.click(egui_phosphor::regular::EYE_SLASH));
            let before = card.components.clone();
            assert!(!card.click(control));
            assert_eq!(card.components, before);
            assert!(card.click(egui_phosphor::regular::EYE));
            assert!(card.click(control));
        }
    }
}

struct PadUi {
    ctx: egui::Context,
    value: [f32; 2],
    enabled: bool,
    time: f64,
    rect: egui::Rect,
    frame: EffectFrame,
    space: controls::PositionSpace,
}

impl PadUi {
    const SPECS: [params::FloatParamSpec; 2] =
        [params::light_rays::SOURCE_X, params::light_rays::SOURCE_Y];

    fn new(value: [f32; 2]) -> Self {
        let ctx = egui::Context::default();
        crate::ui::theme::install(&ctx);
        Self {
            ctx,
            value,
            enabled: true,
            time: 0.0,
            rect: egui::Rect::NOTHING,
            frame: EffectFrame::uncropped(3000, 2000),
            space: controls::PositionSpace::Source,
        }
    }

    /// The image area the pad draws inside its fixed footprint.
    fn canvas(&self) -> egui::Rect {
        controls::fit_aspect(self.rect.shrink(8.0), self.frame.output_aspect())
    }

    fn frame(&mut self, events: Vec<egui::Event>, elapsed: f64) -> bool {
        self.time += elapsed;
        let mut changed = false;
        let modifiers = events
            .iter()
            .find_map(|event| match event {
                egui::Event::Key { modifiers, .. } => Some(*modifiers),
                _ => None,
            })
            .unwrap_or_default();
        let _ = self.ctx.run_ui(
            egui::RawInput {
                time: Some(self.time),
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(280.0, 400.0),
                )),
                modifiers,
                events,
                ..Default::default()
            },
            |ui| {
                ui.add_enabled_ui(self.enabled, |ui| {
                    let (response, edited) = controls::position_pad(
                        ui,
                        "Source position",
                        &mut self.value,
                        Self::SPECS,
                        &self.frame,
                        self.space,
                    );
                    self.rect = response.rect;
                    assert_eq!(response.changed(), edited);
                    changed = edited;
                });
            },
        );
        changed
    }

    fn key(&mut self, key: egui::Key, modifiers: egui::Modifiers) -> bool {
        self.frame(
            [true, false]
                .map(|pressed| egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers,
                })
                .to_vec(),
            0.1,
        )
    }

    fn click(&mut self, target: egui::Pos2) -> bool {
        let changed = self.frame(pointer(target, true), 0.5);
        self.frame(pointer(target, false), 0.1) | changed
    }
}

#[test]
fn position_pad_edits_both_axes_keeps_off_image_values_and_resets_to_spec() {
    let mut pad = PadUi::new([-25.0, 130.0]);
    assert!(!pad.frame(Vec::new(), 0.1));
    assert_eq!(pad.value, [-25.0, 130.0]);
    let canvas = pad.canvas();
    let target = egui::pos2(
        egui::lerp(canvas.x_range(), 0.25),
        egui::lerp(canvas.y_range(), 0.75),
    );
    assert!(pad.click(target));
    assert!((pad.value[0] - 25.0).abs() < 0.01);
    assert!((pad.value[1] - 75.0).abs() < 0.01);
    assert!(pad.key(egui::Key::ArrowRight, egui::Modifiers::NONE));
    assert!((pad.value[0] - 26.0).abs() < 0.01);
    pad.frame(pointer(target, true), 0.8);
    pad.frame(pointer(target, false), 0.04);
    pad.frame(pointer(target, true), 0.04);
    assert!(pad.frame(pointer(target, false), 0.04));
    assert_eq!(pad.value, PadUi::SPECS.map(|spec| spec.default));
}

#[test]
fn position_pad_drag_stops_at_frame_but_keyboard_can_place_off_image_sources() {
    let mut pad = PadUi::new([50.0, 50.0]);
    pad.frame(Vec::new(), 0.1);
    let target = pad.rect.right_top() + egui::vec2(80.0, -80.0);
    pad.frame(pointer(pad.rect.center(), true), 0.5);
    assert!(pad.frame(vec![egui::Event::PointerMoved(target)], 0.1));
    assert!((pad.value[0] - 100.0).abs() < 1e-3 && pad.value[1].abs() < 1e-3);
    pad.value = [100.0, 0.0];
    pad.frame(pointer(target, false), 0.1);
    assert!(pad.key(egui::Key::ArrowRight, egui::Modifiers::SHIFT));
    assert!(pad.key(egui::Key::ArrowUp, egui::Modifiers::NONE));
    assert_eq!(pad.value, [110.0, -1.0]);
    pad.value = [PadUi::SPECS[0].max - 1.0, PadUi::SPECS[1].min + 1.0];
    assert!(pad.key(egui::Key::ArrowRight, egui::Modifiers::SHIFT));
    assert!(pad.key(egui::Key::ArrowUp, egui::Modifiers::SHIFT));
    assert_eq!(pad.value, [PadUi::SPECS[0].max, PadUi::SPECS[1].min]);
    assert!(!pad.key(egui::Key::ArrowRight, egui::Modifiers::SHIFT));
    assert!(!pad.key(egui::Key::ArrowUp, egui::Modifiers::SHIFT));
}

#[test]
fn position_pad_has_the_cropped_preview_shape_and_maps_into_the_crop() {
    let mut pad = PadUi::new([50.0, 50.0]);
    pad.frame.geometry.crop = [0.5, 0.0, 1.0, 1.0];
    pad.frame(Vec::new(), 0.1);
    let full_size = pad.rect;
    let canvas = pad.canvas();
    // A 3:2 source cropped to its right half is 3:4, which the pad shows
    // inside the same footprint.
    assert!((canvas.width() / canvas.height() - 0.75).abs() < 1e-3);
    assert!(canvas.height() <= full_size.shrink(8.0).height() + 1e-3);
    assert!(pad.click(canvas.left_top()));
    assert!((pad.value[0] - 50.0).abs() < 0.01, "{:?}", pad.value);
    assert!(pad.value[1].abs() < 0.01, "{:?}", pad.value);
    assert!(pad.click(canvas.center()));
    assert!((pad.value[0] - 75.0).abs() < 0.01, "{:?}", pad.value);

    // Vignette centers are measured in the cropped frame itself.
    pad.space = controls::PositionSpace::Output;
    assert!(pad.click(canvas.left_top()));
    assert!(pad.value[0].abs() < 0.01 && pad.value[1].abs() < 0.01);
    pad.frame(Vec::new(), 0.1);
    assert_eq!(pad.rect, full_size);
}

#[test]
fn disabled_position_pad_ignores_pointer_and_keyboard_input() {
    let mut pad = PadUi::new([20.0, 70.0]);
    pad.frame(Vec::new(), 0.1);
    assert!(pad.click(pad.rect.center()));
    let before = pad.value;
    pad.enabled = false;
    assert!(!pad.key(egui::Key::ArrowRight, egui::Modifiers::NONE));
    assert!(!pad.key(egui::Key::ArrowUp, egui::Modifiers::SHIFT));
    assert!(!pad.click(pad.rect.left_top() + egui::vec2(12.0, 12.0)));
    assert_eq!(pad.value, before);
    pad.enabled = true;
    // egui hit testing uses the previous frame's enabled widgets. Paint the
    // re-enabled pad before sending a new pointer gesture, as the app does.
    pad.frame(Vec::new(), 0.1);
    assert!(pad.click(pad.rect.left_top() + egui::vec2(12.0, 12.0)));
}

#[test]
fn simple_effects_expose_all_controls_without_extra_sections() {
    for (effect, labels) in [
        (
            MaskEffect::Blur,
            vec![params::blur::AMOUNT.label, params::blur::RADIUS.label],
        ),
        (
            MaskEffect::MotionBlur,
            vec![
                params::motion_blur::AMOUNT.label,
                params::motion_blur::DISTANCE.label,
                params::motion_blur::ANGLE.label,
            ],
        ),
        (
            MaskEffect::Pixelate,
            vec![
                params::pixelate::AMOUNT.label,
                params::pixelate::BLOCK_SIZE.label,
            ],
        ),
    ] {
        let mut card = CardUi::new(effect, 280.0, true);
        card.frame(Vec::new());
        for label in labels {
            assert!(card.has(label));
        }
        assert!(card.has(effect_description(effect).unwrap()));
        assert_eq!(card.components[0].settings, MaskEffectSettings::default());
    }
}
