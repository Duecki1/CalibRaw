use super::*;

const LENS_CARD_TITLE: &str = "Lens Corrections";

/// Chooses between a Lensfun profile and hand-set corrections.
fn lens_mode_selector(ui: &mut Ui, manual: &mut bool) {
    ui.horizontal(|ui| {
        let width = ((ui.available_width() - ui.spacing().item_spacing.x) * 0.5).max(1.0);
        for (label, candidate) in [("Profile", false), ("Manual", true)] {
            if moduwu_design::segmented_button(ui, label, *manual == candidate, width).clicked() {
                *manual = candidate;
            }
        }
    });
}

impl Sidebar {
    pub(super) fn show_optics(ui: &mut Ui, app: &mut CalibRawApp, foldable: bool) -> bool {
        let mut rebuild = false;
        let mut open_adjust_dialog = false;
        let capture = app.develop.original_raw.as_ref().map(|raw| {
            let lens = match (raw.lens_make.trim(), raw.lens_model.trim()) {
                ("", "") => "Not reported".to_owned(),
                ("", model) => model.to_owned(),
                (maker, "") => maker.to_owned(),
                (maker, model) => format!("{maker} {model}"),
            };
            let focal = (raw.focal_length > 0.0).then(|| format!("{:.1} mm", raw.focal_length));
            let aperture = (raw.aperture > 0.0).then(|| format!("f/{:.1}", raw.aperture));
            (lens, focal, aperture)
        });
        if std::mem::take(&mut app.develop_ui.reveal_lens_card) {
            adjustment_cards::reveal_adjustment_card(ui.ctx(), LENS_CARD_TITLE);
        }

        let action = Self::adjustment_card_without_eye(
            ui,
            LENS_CARD_TITLE,
            false,
            foldable,
            |ui| {
                let lens_correction_busy = app.lens_correction_busy();
                let state = &mut app.develop.lens_correction;
                let can_enable = state.manual.is_some()
                    || (state.catalog.available && state.selected_lens().is_some());
                let enabled_response = ui
                    .add_enabled_ui(can_enable && !lens_correction_busy, |ui| {
                        moduwu_design::toggle_row(ui, &mut state.enabled, "Enabled")
                    })
                    .inner;
                if enabled_response.changed() {
                    rebuild = true;
                }
                if !state.catalog.available && state.manual.is_none() {
                    state.enabled = false;
                    state.applied = false;
                }

                ui.add_space(moduwu_design::SPACE_XXS);
                let mut manual_mode = state.manual.is_some();
                ui.add_enabled_ui(!lens_correction_busy, |ui| {
                    lens_mode_selector(ui, &mut manual_mode)
                });
                if manual_mode != state.manual.is_some() {
                    state.set_manual_mode(manual_mode);
                    rebuild = true;
                }

                ui.add_space(moduwu_design::SPACE_XXS);
                egui::Grid::new("lens-correction-capture-metadata")
                    .num_columns(2)
                    .spacing(egui::vec2(10.0, 3.0))
                    .show(ui, |ui| {
                        ui.label("Camera");
                        ui.label(if state.catalog.camera_label.is_empty() {
                            "Not matched"
                        } else {
                            state.catalog.camera_label.as_str()
                        });
                        ui.end_row();
                        if let Some((lens, focal, aperture)) = &capture {
                            ui.label("RAW lens");
                            ui.label(lens);
                            ui.end_row();
                            if let Some(focal) = focal {
                                ui.label("Focal length");
                                ui.label(focal);
                                ui.end_row();
                            }
                            if let Some(aperture) = aperture {
                                ui.label("Aperture");
                                ui.label(aperture);
                                ui.end_row();
                            }
                        }
                    });

                ui.add_space(moduwu_design::SPACE_XS);
                if state.manual.is_some() {
                    open_adjust_dialog = ui
                        .add_enabled_ui(!lens_correction_busy, |ui| {
                            moduwu_design::full_width_button(ui, "Adjust")
                        })
                        .inner
                        .clicked();
                    return;
                }

                let makers = state.makers();
                let previous_maker = state.selected_maker.clone();
                let selected_maker_text = if state.selected_maker.is_empty() {
                    if state.selected_model.is_empty() {
                        "Select a brand".to_owned()
                    } else {
                        "Unknown".to_owned()
                    }
                } else {
                    state.selected_maker.clone()
                };
                ui.add_enabled_ui(
                    state.catalog.available && !makers.is_empty() && !lens_correction_busy,
                    |ui| {
                        moduwu_design::form_combo(
                            ui,
                            "Brand",
                            "lens-correction-brand",
                            selected_maker_text,
                            240.0,
                            |ui| {
                                for maker in &makers {
                                    ui.selectable_value(
                                        &mut state.selected_maker,
                                        maker.clone(),
                                        if maker.is_empty() { "Unknown" } else { maker },
                                    );
                                }
                            },
                        );
                    },
                );
                let mut selection_changed = state.selected_maker != previous_maker;
                if selection_changed {
                    let first_model = state
                        .models_for_maker(&state.selected_maker)
                        .into_iter()
                        .next()
                        .unwrap_or_default();
                    state.selected_model = first_model;
                }

                let models = state.models_for_maker(&state.selected_maker);
                let previous_model = state.selected_model.clone();
                let selected_model_text = if state.selected_model.is_empty() {
                    "Select a lens".to_owned()
                } else {
                    state.selected_model.clone()
                };
                ui.add_enabled_ui(
                    state.catalog.available && !models.is_empty() && !lens_correction_busy,
                    |ui| {
                        moduwu_design::form_combo(
                            ui,
                            "Lens",
                            "lens-correction-model",
                            selected_model_text,
                            240.0,
                            |ui| {
                                for model in &models {
                                    ui.selectable_value(
                                        &mut state.selected_model,
                                        model.clone(),
                                        model,
                                    );
                                }
                            },
                        );
                    },
                );
                selection_changed |= state.selected_model != previous_model;

                ui.add_space(moduwu_design::SPACE_XS);
                let previous_corrections = state.corrections;
                ui.add_enabled_ui(state.catalog.available && !lens_correction_busy, |ui| {
                    moduwu_design::toggle_with_help(
                        ui,
                        &mut state.corrections.geometry,
                        "Geometry",
                        "Correct lens distortion and colour fringing (lateral chromatic aberration).",
                    );
                    moduwu_design::toggle_with_help(
                        ui,
                        &mut state.corrections.vignetting,
                        "Vignetting",
                        "Brighten the corners the lens darkens.",
                    );
                });
                // At least one correction must stay selected.
                if !state.corrections.geometry && !state.corrections.vignetting {
                    state.corrections = previous_corrections;
                }
                selection_changed |= state.corrections != previous_corrections;
                if selection_changed {
                    state.applied = false;
                    if let Some(selection) = state.selected_lens() {
                        state.catalog.status = if state.enabled {
                            format!("Applying {}…", selection.label())
                        } else {
                            format!(
                                "Selected {}. Enable correction to apply it.",
                                selection.label()
                            )
                        };
                    }
                    if state.enabled {
                        rebuild = true;
                    }
                }
            },
        );
        match action {
            adjustment_cards::CardAction::None => {}
            adjustment_cards::CardAction::Toggle => {}
            adjustment_cards::CardAction::Reset => {
                let automatic = app.preferences.automatic_lens_correction;
                let state = &mut app.develop.lens_correction;
                *state =
                    crate::app::LensCorrectionState::automatic(state.catalog.clone(), automatic);
                rebuild = true;
            }
        }
        if open_adjust_dialog {
            app.open_manual_lens_dialog();
        }
        rebuild
    }

    pub(super) fn show_basic(ui: &mut Ui, exposure: &mut ExposureParams, foldable: bool) -> bool {
        let mut changed = false;
        let action = Self::adjustment_card(ui, "Light", true, foldable, true, |ui| {
            changed |= AdjustmentSlider::new("Exposure", &mut exposure.exposure, -5.0..=5.0)
                .decimals(2)
                .step(0.05)
                .gradient(SliderGradient::Brightness)
                .show(ui);
            changed |= AdjustmentSlider::new("Contrast", &mut exposure.contrast, -100.0..=100.0)
                .decimals(0)
                .step(1.0)
                .gradient(SliderGradient::Brightness)
                .show(ui);
            changed |=
                AdjustmentSlider::new("Highlights", &mut exposure.highlights, -100.0..=100.0)
                    .decimals(0)
                    .step(1.0)
                    .gradient(SliderGradient::Brightness)
                    .show(ui);
            changed |= AdjustmentSlider::new("Shadows", &mut exposure.shadows, -100.0..=100.0)
                .decimals(0)
                .step(1.0)
                .gradient(SliderGradient::Brightness)
                .show(ui);
            changed |= AdjustmentSlider::new("Whites", &mut exposure.whites, -100.0..=100.0)
                .decimals(0)
                .step(1.0)
                .gradient(SliderGradient::Brightness)
                .show(ui);
            changed |= AdjustmentSlider::new("Blacks", &mut exposure.blacks, -100.0..=100.0)
                .decimals(0)
                .step(1.0)
                .gradient(SliderGradient::Brightness)
                .show(ui);
        });
        changed |= action.apply(exposure, AdjustmentGroup::Light);
        changed
    }

    pub(super) fn show_tone_curve(
        ui: &mut Ui,
        exposure: &mut ExposureParams,
        selected_tab: &mut ToneCurveTab,
        foldable: bool,
    ) -> bool {
        let mut changed = false;
        let action = Self::adjustment_card(ui, "Tone Curve", false, foldable, true, |ui| {
            changed |= tone_curve_channel_editor(
                ui,
                ToneCurveChannels {
                    rgb: &mut exposure.tone_curve,
                    red: &mut exposure.tone_curve_red,
                    green: &mut exposure.tone_curve_green,
                    blue: &mut exposure.tone_curve_blue,
                },
                selected_tab,
                4.0,
            );
        });
        changed |= action.apply(exposure, AdjustmentGroup::ToneCurve);
        changed
    }

    pub(super) fn show_color(
        ui: &mut Ui,
        exposure: &mut ExposureParams,
        raw: Option<&LoadedRaw>,
        white_balance_picker_active: &mut bool,
        foldable: bool,
    ) -> bool {
        let mut changed = false;
        let action = Self::adjustment_card(ui, "Color", false, foldable, true, |ui| {
            if let Some(raw) = raw.filter(|raw| {
                raw.white_balance_temperature_tint(exposure.temperature, exposure.tint)
                    .is_some()
            }) {
                let presets = raw.camera_white_balance_presets();
                let matches_current = |candidate: (f32, f32)| {
                    (candidate.0 - exposure.temperature).abs() < 0.01
                        && (candidate.1 - exposure.tint).abs() < 0.01
                };
                let selection = if *white_balance_picker_active {
                    "from image area".to_owned()
                } else if exposure.temperature.abs() < 1e-5 && exposure.tint.abs() < 1e-5 {
                    "as shot".to_owned()
                } else if raw
                    .white_balance_offsets_from_temperature_tint(6504.0, 1.0)
                    .is_some_and(&matches_current)
                {
                    "camera reference (D65)".to_owned()
                } else if let Some(preset) = presets.iter().find(|preset| {
                    raw.white_balance_offsets_from_coefficients(preset.coefficients)
                        .is_some_and(&matches_current)
                }) {
                    preset.name.clone()
                } else if let Some(temperature) = [2500.0, 3200.0, 4500.0, 6000.0, 8500.0]
                    .into_iter()
                    .find(|temperature| {
                        raw.white_balance_offsets_from_temperature_tint(*temperature, 1.0)
                            .is_some_and(&matches_current)
                    })
                {
                    format!("{temperature:.0}K")
                } else {
                    "user modified".to_owned()
                };
                ui.horizontal(|ui| {
                    let picker_width = moduwu_design::TOOLBAR_ICON_EDGE;
                    let combo_width =
                        (ui.available_width() - picker_width - ui.spacing().item_spacing.x)
                            .clamp(1.0, 240.0);
                    moduwu_design::combo_box("global-white-balance-preset", selection, combo_width)
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(false, "as shot").clicked() {
                                exposure.temperature = 0.0;
                                exposure.tint = 0.0;
                                *white_balance_picker_active = false;
                                changed = true;
                            }
                            if ui.selectable_label(false, "from image area").clicked() {
                                *white_balance_picker_active = true;
                            }
                            ui.label(
                                egui::RichText::new("reference")
                                    .strong()
                                    .color(ui.visuals().weak_text_color()),
                            );
                            if ui
                                .selectable_label(false, "camera reference (D65)")
                                .clicked()
                            {
                                if let Some((temperature, tint)) =
                                    raw.white_balance_offsets_from_temperature_tint(6504.0, 1.0)
                                {
                                    exposure.temperature = temperature;
                                    exposure.tint = tint;
                                    *white_balance_picker_active = false;
                                    changed = true;
                                }
                            }
                            if !presets.is_empty() {
                                ui.separator();
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} {}",
                                        raw.camera_make, raw.camera_model
                                    ))
                                    .strong(),
                                );
                                for preset in &presets {
                                    if ui.selectable_label(false, &preset.name).clicked() {
                                        if let Some((temperature, tint)) = raw
                                            .white_balance_offsets_from_coefficients(
                                                preset.coefficients,
                                            )
                                        {
                                            exposure.temperature = temperature;
                                            exposure.tint = tint;
                                            *white_balance_picker_active = false;
                                            changed = true;
                                        }
                                    }
                                }
                            }
                            ui.separator();
                            ui.label(
                                egui::RichText::new("fixed temperature")
                                    .strong()
                                    .color(ui.visuals().weak_text_color()),
                            );
                            for temperature in [2500.0, 3200.0, 4500.0, 6000.0, 8500.0] {
                                if ui
                                    .selectable_label(false, format!("{temperature:.0}K"))
                                    .clicked()
                                {
                                    if let Some((temperature, tint)) = raw
                                        .white_balance_offsets_from_temperature_tint(
                                            temperature,
                                            1.0,
                                        )
                                    {
                                        exposure.temperature = temperature;
                                        exposure.tint = tint;
                                        *white_balance_picker_active = false;
                                        changed = true;
                                    }
                                }
                            }
                        });
                    let picker = moduwu_design::ghost_icon_toggle_button(
                        ui,
                        egui_phosphor::regular::EYEDROPPER,
                        *white_balance_picker_active,
                        egui::vec2(picker_width, moduwu_design::CONTROL_HEIGHT),
                        "Pick a neutral gray or white area in the image",
                    );
                    if picker.clicked() {
                        *white_balance_picker_active = !*white_balance_picker_active;
                    }
                });
                if *white_balance_picker_active {
                    ui.label(
                        egui::RichText::new("Drag over a neutral area in the image")
                            .size(11.5)
                            .color(ui.visuals().selection.bg_fill),
                    );
                }

                let (mut kelvin, mut tint) = raw
                    .white_balance_temperature_tint(exposure.temperature, exposure.tint)
                    .expect("white-balance model was checked above");
                let ranges = raw
                    .white_balance_ranges()
                    .expect("white-balance ranges were checked above");
                let base_kelvin = raw.as_shot_temperature_kelvin().unwrap_or(kelvin);
                let kelvin_changed = ui
                    .push_id(base_kelvin.to_bits(), |ui| {
                        AdjustmentSlider::new(
                            "Temperature (K)",
                            &mut kelvin,
                            ranges.temperature.clone(),
                        )
                        .decimals(0)
                        .step(10.0)
                        .gradient(SliderGradient::Temperature)
                        .reset_to(base_kelvin)
                        .show(ui)
                    })
                    .inner;
                let base_tint = raw.as_shot_white_balance().map_or(tint, |value| value.1);
                let tint_range = ranges.tint;
                let tint_neutral_fraction = ((1.0 - tint_range.start())
                    / (tint_range.end() - tint_range.start()).max(f32::EPSILON))
                .clamp(0.0, 1.0);
                let tint_changed = ui
                    .push_id(base_tint.to_bits(), |ui| {
                        AdjustmentSlider::new("Tint", &mut tint, tint_range)
                            .decimals(3)
                            .step(0.005)
                            .gradient(SliderGradient::CameraTint {
                                neutral_fraction: tint_neutral_fraction,
                            })
                            .reset_to(base_tint)
                            .show(ui)
                    })
                    .inner;
                if kelvin_changed || tint_changed {
                    if let Some((temperature, tint)) =
                        raw.white_balance_offsets_from_temperature_tint(kelvin, tint)
                    {
                        // Each slider writes only its own value, so moving one
                        // never changes the other.
                        if kelvin_changed {
                            exposure.temperature = temperature;
                        }
                        if tint_changed {
                            exposure.tint = tint;
                        }
                        *white_balance_picker_active = false;
                        changed = true;
                    }
                }
            } else {
                *white_balance_picker_active = false;
                ui.label("White balance");
                ui.label(
                    egui::RichText::new(
                        "Unavailable: this image has no usable white-balance metadata",
                    )
                    .size(11.5)
                    .color(ui.visuals().weak_text_color()),
                );
            }
            changed |= hue_adjustment_slider(ui, &mut exposure.hue);
            changed |= AdjustmentSlider::new("Vibrance", &mut exposure.vibrance, -100.0..=100.0)
                .decimals(0)
                .step(1.0)
                .gradient(SliderGradient::Colorfulness)
                .show(ui);
            changed |=
                AdjustmentSlider::new("Saturation", &mut exposure.saturation, -100.0..=100.0)
                    .decimals(0)
                    .step(1.0)
                    .gradient(SliderGradient::Colorfulness)
                    .show(ui);
        });
        changed |= action.apply(exposure, AdjustmentGroup::Color);
        if !crate::app::preview_visibility::PreviewVisibility::visible(ui.ctx(), "Color")
            || !matches!(action, adjustment_cards::CardAction::None)
        {
            *white_balance_picker_active = false;
        }
        changed
    }

    pub(super) fn show_color_grading(
        ui: &mut Ui,
        exposure: &mut ExposureParams,
        selected_tab: &mut ColorGradeTab,
        foldable: bool,
    ) -> bool {
        let mut changed = false;
        let action = Self::adjustment_card(ui, "Color Grading", false, foldable, true, |ui| {
            changed |= color_grading_editor(ui, &mut exposure.color_grading, selected_tab);
        });
        changed |= action.apply(exposure, AdjustmentGroup::ColorGrading);
        changed
    }

    /// `ai_strength_available` is whether the photo has an AI result that can
    /// be blended with the original: a Bayer RAW whose denoised mosaic is loaded.
    pub(super) fn show_detail(
        ui: &mut Ui,
        exposure: &mut ExposureParams,
        ai_strength_available: bool,
        foldable: bool,
    ) -> (bool, Option<bool>) {
        let mut changed = false;
        let mut ai_request = None;
        let ai_before = exposure.ai_denoise_enabled;
        let action = Self::adjustment_card(ui, "Detail", false, foldable, true, |ui| {
            let mut ai_enabled = exposure.ai_denoise_enabled;
            if moduwu_design::toggle_with_help(
                ui,
                &mut ai_enabled,
                "AI Denoise — RawNIND UtNet2",
                "Runs the pinned darktable-ai RawNIND model locally. Bayer uses joint denoise/demosaic; X-Trans uses the linear Rec.2020 variant.",
            )
            .changed()
            {
                ai_request = Some(ai_enabled);
            }
            if ai_strength_available && exposure.ai_denoise_enabled {
                let mut strength_percent = exposure.ai_denoise_amount.clamp(0.0, 1.0) * 100.0;
                if AdjustmentSlider::new("Strength", &mut strength_percent, 0.0..=100.0)
                    .decimals(0)
                    .step(1.0)
                    .reset_to(ExposureParams::default().ai_denoise_amount * 100.0)
                    .show(ui)
                {
                    exposure.ai_denoise_amount = strength_percent / 100.0;
                    changed = true;
                }
            }
            moduwu_design::section_separator(ui);
            moduwu_design::strong_with_help(
                ui,
                "Noise reduction",
                "Sensor-profiled noise reduction uses the RAW's estimated a·signal+b sensor model. AI Denoise replaces these manual controls while enabled.",
            );
            ui.add_enabled_ui(!exposure.ai_denoise_enabled, |ui| {
                changed |= AdjustmentSlider::new(
                    "Luminance",
                    &mut exposure.luminance_denoise,
                    0.0..=100.0,
                )
                .decimals(0)
                .step(1.0)
                .show(ui);
                let mut color_percent = exposure.chroma_denoise.clamp(0.0, 1.0) * 100.0;
                if AdjustmentSlider::new("Color", &mut color_percent, 0.0..=100.0)
                    .decimals(0)
                    .step(1.0)
                    .show(ui)
                {
                    exposure.chroma_denoise = color_percent / 100.0;
                    changed = true;
                }
                changed |= AdjustmentSlider::new(
                    "Denoise Detail",
                    &mut exposure.denoise_detail,
                    0.0..=100.0,
                )
                .decimals(0)
                .step(1.0)
                .reset_to(ExposureParams::default().denoise_detail)
                .show(ui);
                let previous_quality = exposure.denoise_quality;
                moduwu_design::form_combo(
                    ui,
                    "Denoise quality",
                    "develop-denoise-quality",
                    exposure.denoise_quality.label(),
                    150.0,
                    |ui| {
                        ui.selectable_value(
                            &mut exposure.denoise_quality,
                            DenoiseQuality::Fast,
                            DenoiseQuality::Fast.label(),
                        );
                        ui.selectable_value(
                            &mut exposure.denoise_quality,
                            DenoiseQuality::Balanced,
                            DenoiseQuality::Balanced.label(),
                        );
                        ui.selectable_value(
                            &mut exposure.denoise_quality,
                            DenoiseQuality::High,
                            DenoiseQuality::High.label(),
                        );
                    },
                );
                changed |= previous_quality != exposure.denoise_quality;
            });
            moduwu_design::section_separator(ui);
            moduwu_design::strong_with_help(
                ui,
                "Capture sharpening",
                "Edge-aware capture sharpening restores fine RAW detail while its radius, detail, and masking controls limit halos and noisy texture.",
            );
            changed |= AdjustmentSlider::new("Amount", &mut exposure.sharpen_amount, 0.0..=150.0)
                .decimals(0)
                .step(1.0)
                .reset_to(ExposureParams::default().sharpen_amount)
                .show(ui);
            changed |= AdjustmentSlider::new("Radius", &mut exposure.sharpen_radius, 0.5..=3.0)
                .decimals(2)
                .step(0.05)
                .reset_to(ExposureParams::default().sharpen_radius)
                .show(ui);
            changed |= AdjustmentSlider::new("Detail", &mut exposure.sharpen_detail, 0.0..=100.0)
                .decimals(0)
                .step(1.0)
                .reset_to(ExposureParams::default().sharpen_detail)
                .show(ui);
            changed |= AdjustmentSlider::new("Masking", &mut exposure.sharpen_masking, 0.0..=100.0)
                .decimals(0)
                .step(1.0)
                .show(ui);
        });
        changed |= action.apply(exposure, AdjustmentGroup::Detail);
        if exposure.ai_denoise_enabled != ai_before {
            ai_request = Some(exposure.ai_denoise_enabled);
            exposure.ai_denoise_enabled = ai_before;
        }
        (changed, ai_request)
    }

    /// The Effects card. Returns whether a slider changed and the effect a
    /// button asked to add to `global_effects`.
    pub(super) fn show_presence(
        ui: &mut Ui,
        exposure: &mut ExposureParams,
        global_effects: &[crate::pipeline::EffectComponent],
        foldable: bool,
    ) -> (bool, Option<MaskEffect>) {
        let mut changed = false;
        let mut added = None;
        let action = Self::adjustment_card(ui, "Effects", false, foldable, true, |ui| {
            changed |= AdjustmentSlider::new("Texture", &mut exposure.texture, -100.0..=100.0)
                .decimals(0)
                .step(1.0)
                .show(ui);
            changed |= AdjustmentSlider::new("Clarity", &mut exposure.clarity, -100.0..=100.0)
                .decimals(0)
                .step(1.0)
                .show(ui);
            changed |= AdjustmentSlider::new("Dehaze", &mut exposure.dehaze, -100.0..=100.0)
                .decimals(0)
                .step(1.0)
                .show(ui);

            moduwu_design::section_separator(ui);
            added =
                mask_effects::add_effect_buttons(ui, &mask_effects::FINISH_EFFECTS, global_effects);
        });
        changed |= action.apply(exposure, AdjustmentGroup::Effects);
        (changed, added)
    }

    pub(super) fn show_hsl(
        ui: &mut Ui,
        exposure: &mut ExposureParams,
        selected_color: &mut HslMixerColor,
        point_color: &mut crate::ui::components::point_color::PointColorUiState,
        point_color_tab: &mut bool,
        foldable: bool,
    ) -> bool {
        let mut changed = false;
        let action = Self::adjustment_card(ui, "Color Mixer", false, foldable, true, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(point_color_tab, false, "Mixer");
                ui.selectable_value(point_color_tab, true, "Point Color");
            });
            ui.add_space(moduwu_design::SPACE_XS);
            if *point_color_tab {
                changed |= crate::ui::components::point_color::point_color(
                    ui,
                    &mut exposure.point_colors,
                    point_color,
                );
            } else {
                point_color.picker_active = false;
                point_color.visualize_range = false;
                changed |= hsl_mixer(
                    ui,
                    selected_color,
                    &mut exposure.hsl_hue,
                    &mut exposure.hsl_saturation,
                    &mut exposure.hsl_luminance,
                );
            }
        });
        changed |= action.apply(exposure, AdjustmentGroup::ColorMixer);
        changed
    }
}
