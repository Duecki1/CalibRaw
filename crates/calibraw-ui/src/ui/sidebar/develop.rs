use super::*;

impl Sidebar {
    pub(super) fn show_optics(ui: &mut Ui, app: &mut CalibRawApp, foldable: bool) -> bool {
        let mut rebuild = false;
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

        let action = Self::adjustment_card(ui, "Lens Corrections", false, foldable, true, |ui| {
            let lens_correction_busy = app.lens_correction_busy();
            let state = &mut app.develop.lens_correction;
            let has_selection = state.selected_lens().is_some();
            let enabled_response = ui
                .add_enabled_ui(
                    state.catalog.available && has_selection && !lens_correction_busy,
                    |ui| moduwu_design::toggle(ui, &mut state.enabled, "Enabled"),
                )
                .inner;
            if enabled_response.changed() {
                rebuild = true;
            }
            if !state.catalog.available {
                state.enabled = false;
                state.applied = false;
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
                ui.horizontal_wrapped(|ui| {
                    moduwu_design::toggle(ui, &mut state.corrections.geometry, "Geometry")
                        .on_hover_text(
                            "Correct lens distortion and colour fringing (lateral chromatic aberration).",
                        );
                    moduwu_design::toggle(ui, &mut state.corrections.vignetting, "Vignetting")
                        .on_hover_text("Brighten the corners the lens darkens.");
                });
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
        });
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
        rebuild
    }

    /// The colour look: a `.cube` lookup table applied after tone mapping.
    /// Importing is desktop-only, so Android shows the card only for a look
    /// that came with a preset or sidecar.
    pub(super) fn show_color_lut(ui: &mut Ui, app: &mut CalibRawApp, foldable: bool) {
        #[cfg(target_os = "android")]
        if app.develop.color_lut.is_none() {
            return;
        }
        let can_edit = app.can_edit_color_lut();
        let mut amount_changed = false;
        #[cfg(not(target_os = "android"))]
        let mut import_requested = false;
        let mut remove_requested = false;
        let action = Self::tool_card(ui, "Look (LUT)", false, foldable, can_edit, |ui| {
            match app.develop.color_lut.as_mut() {
                Some(look) => {
                    ui.strong(&look.name);
                    ui.small(format!("{0}×{0}×{0} colour lookup table", look.lut.edge()));
                    amount_changed |=
                        AdjustmentSlider::new("Amount", &mut look.amount, 0.0..=100.0)
                            .decimals(0)
                            .step(1.0)
                            .hover_text("How much of the look is mixed into the image.")
                            .reset_to(crate::pipeline::FULL_COLOR_LUT_AMOUNT)
                            .show(ui);
                }
                None => {
                    ui.small(
                        "Apply a film or creative look from a .cube lookup table after tone mapping.",
                    );
                }
            }
            #[cfg(not(target_os = "android"))]
            ui.horizontal_wrapped(|ui| {
                let label = if app.develop.color_lut.is_some() {
                    "Replace…"
                } else {
                    "Import .cube…"
                };
                import_requested = moduwu_design::secondary_button_enabled(ui, can_edit, label)
                    .on_hover_text("Choose a 3D .cube lookup table. 1D tables are not supported.")
                    .clicked();
                if app.develop.color_lut.is_some() {
                    remove_requested =
                        moduwu_design::secondary_button_enabled(ui, can_edit, "Remove")
                            .on_hover_text("Remove the look from this photo.")
                            .clicked();
                }
            });
            #[cfg(target_os = "android")]
            {
                remove_requested = moduwu_design::secondary_button_enabled(ui, can_edit, "Remove")
                    .on_hover_text("Remove the look from this photo.")
                    .clicked();
            }
        });
        if matches!(action, adjustment_cards::CardAction::Reset) {
            remove_requested = true;
        }
        #[cfg(not(target_os = "android"))]
        if import_requested {
            app.choose_color_lut_to_import();
        }
        if remove_requested && app.develop.color_lut.is_some() {
            app.set_color_lut(None);
        } else if amount_changed {
            app.mark_color_lut_dirty();
        }
    }

    pub(super) fn show_basic(ui: &mut Ui, exposure: &mut ExposureParams, foldable: bool) -> bool {
        let mut changed = false;
        let action = Self::adjustment_card(ui, "Light", true, foldable, true, |ui| {
            changed |= AdjustmentSlider::new("Exposure", &mut exposure.exposure, -5.0..=5.0)
                .decimals(2)
                .step(0.05)
                .hover_text("Overall scene-linear brightness in exposure stops.")
                .gradient(SliderGradient::Brightness)
                .show(ui);
            changed |= AdjustmentSlider::new("Contrast", &mut exposure.contrast, -100.0..=100.0)
                .decimals(0)
                .step(1.0)
                .hover_text("Maps -100%..+100% to darktable's normal sigmoid contrast range, 0.7..3.0 around its 1.5 default.")
                .gradient(SliderGradient::Brightness)
                .show(ui);
            changed |=
                AdjustmentSlider::new("Highlights", &mut exposure.highlights, -100.0..=100.0)
                    .decimals(0)
                    .step(1.0)
                    .hover_text(
                        "Recovers or brightens the upper tonal range without hard clipping.",
                    )
                    .gradient(SliderGradient::Brightness)
                    .show(ui);
            changed |= AdjustmentSlider::new("Shadows", &mut exposure.shadows, -100.0..=100.0)
                .decimals(0)
                .step(1.0)
                .hover_text("Opens or deepens the lower tonal range.")
                .gradient(SliderGradient::Brightness)
                .show(ui);
            changed |= AdjustmentSlider::new("Whites", &mut exposure.whites, -100.0..=100.0)
                .decimals(0)
                .step(1.0)
                .hover_text("Moves the bright endpoint and specular range.")
                .gradient(SliderGradient::Brightness)
                .show(ui);
            changed |= AdjustmentSlider::new("Blacks", &mut exposure.blacks, -100.0..=100.0)
                .decimals(0)
                .step(1.0)
                .hover_text("Moves the display black/toe endpoint while preserving sensor black calibration.")
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
                    let picker = moduwu_design::icon_toggle_button(
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
                let base_kelvin = raw.as_shot_temperature_kelvin().unwrap_or(kelvin);
                let temperature_range = raw
                    .white_balance_temperature_range(kelvin, tint)
                    .unwrap_or(kelvin..=kelvin);
                let kelvin_changed = ui
                    .push_id(base_kelvin.to_bits(), |ui| {
                        AdjustmentSlider::new("Temperature (K)", &mut kelvin, temperature_range)
                            .decimals(0)
                            .step(10.0)
                            .hover_text("Scene illuminant color temperature in Kelvin. The range follows the camera's valid white balance at the current tint; double-click to reset toward the as-shot temperature.")
                            .gradient(SliderGradient::Temperature)
                            .reset_to(base_kelvin)
                            .show(ui)
                    })
                    .inner;
                let base_tint = raw.as_shot_white_balance().map_or(tint, |value| value.1);
                let tint_range = raw.white_balance_tint_range(kelvin).unwrap_or(tint..=tint);
                let tint_neutral_fraction = ((1.0 - tint_range.start())
                    / (tint_range.end() - tint_range.start()).max(f32::EPSILON))
                .clamp(0.0, 1.0);
                let tint_changed = ui
                    .push_id(base_tint.to_bits(), |ui| {
                        AdjustmentSlider::new("Tint", &mut tint, tint_range)
                            .decimals(3)
                            .step(0.005)
                            .hover_text("Camera tint: lower values add magenta, higher values add green. The range follows the camera's valid white balance at the current temperature; double-click to reset toward the as-shot tint.")
                            .gradient(SliderGradient::CameraTint { neutral_fraction: tint_neutral_fraction })
                            .reset_to(base_tint)
                            .show(ui)
                    })
                    .inner;
                if kelvin_changed || tint_changed {
                    if let Some((temperature, tint)) =
                        raw.white_balance_offsets_from_temperature_tint(kelvin, tint)
                    {
                        exposure.temperature = temperature;
                        exposure.tint = tint;
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
            changed |= hue_adjustment_slider(
                ui,
                &mut exposure.hue,
                Some("Rotates every color around the perceptual color wheel while preserving lightness and chroma."),
            );
            changed |= AdjustmentSlider::new("Vibrance", &mut exposure.vibrance, -100.0..=100.0)
                .decimals(0)
                .step(1.0)
                .hover_text(
                    "Perceptual colorfulness with protection for saturated colors and skin hues.",
                )
                .gradient(SliderGradient::Colorfulness)
                .show(ui);
            changed |=
                AdjustmentSlider::new("Saturation", &mut exposure.saturation, -100.0..=100.0)
                    .decimals(0)
                    .step(1.0)
                    .hover_text("Uniform perceptual chroma scaling.")
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

    pub(super) fn show_detail(
        ui: &mut Ui,
        exposure: &mut ExposureParams,
        foldable: bool,
    ) -> (bool, Option<bool>) {
        let mut changed = false;
        let mut ai_request = None;
        let ai_before = exposure.ai_denoise_enabled;
        let action = Self::adjustment_card(ui, "Detail", false, foldable, true, |ui| {
            let mut ai_enabled = exposure.ai_denoise_enabled;
            let ai_response =
                moduwu_design::toggle(ui, &mut ai_enabled, "AI Denoise — RawNIND UtNet2");
            if ai_response.changed() {
                ai_request = Some(ai_enabled);
            }
            ai_response.on_hover_text(
                "Runs the pinned darktable-ai RawNIND model locally. Bayer uses joint denoise/demosaic; X-Trans uses the linear Rec.2020 variant.",
            );
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
                    .hover_text("Reduces shot/read noise using the RAW's estimated a·signal+b sensor model. Higher values can smooth fine texture.")
                    .show(ui);
                let mut color_percent = exposure.chroma_denoise.clamp(0.0, 1.0) * 100.0;
                if AdjustmentSlider::new("Color", &mut color_percent, 0.0..=100.0)
                    .decimals(0)
                    .step(1.0)
                    .hover_text("Reduces color speckling while keeping luminance structure comparatively intact.")
                    .show(ui) {
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
                    .hover_text("Higher values protect edges and microtexture more strongly; lower values permit smoother denoising.")
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
                .hover_text("Controls overall capture sharpening strength. Zero is an exact no-op.")
                .reset_to(ExposureParams::default().sharpen_amount)
                .show(ui);
            changed |= AdjustmentSlider::new("Radius", &mut exposure.sharpen_radius, 0.5..=3.0)
                .decimals(2)
                .step(0.05)
                .hover_text("Controls the edge width being sharpened. Smaller values favor fine detail; larger values strengthen broader edges.")
                .reset_to(ExposureParams::default().sharpen_radius)
                .show(ui);
            changed |= AdjustmentSlider::new("Detail", &mut exposure.sharpen_detail, 0.0..=100.0)
                .decimals(0)
                .step(1.0)
                .hover_text("Raises the contribution of the finest texture and lowers fine-detail suppression.")
                .reset_to(ExposureParams::default().sharpen_detail)
                .show(ui);
            changed |= AdjustmentSlider::new("Masking", &mut exposure.sharpen_masking, 0.0..=100.0)
                .decimals(0)
                .step(1.0)
                .hover_text("Restricts sharpening to stronger luminance edges as the value increases, protecting flat areas and noise.")
                .show(ui);
        });
        changed |= action.apply(exposure, AdjustmentGroup::Detail);
        if exposure.ai_denoise_enabled != ai_before {
            ai_request = Some(exposure.ai_denoise_enabled);
            exposure.ai_denoise_enabled = ai_before;
        }
        (changed, ai_request)
    }

    pub(super) fn show_presence(
        ui: &mut Ui,
        exposure: &mut ExposureParams,
        foldable: bool,
    ) -> bool {
        let mut changed = false;
        let action = Self::adjustment_card(ui, "Effects", false, foldable, true, |ui| {
            changed |= AdjustmentSlider::new("Texture", &mut exposure.texture, -100.0..=100.0)
                .decimals(0)
                .step(1.0)
                .hover_text(
                    "Enhances or softens fine surface detail without changing overall exposure.",
                )
                .show(ui);
            changed |= AdjustmentSlider::new("Clarity", &mut exposure.clarity, -100.0..=100.0)
                .decimals(0)
                .step(1.0)
                .hover_text("Changes edge-aware midtone local contrast while protecting highlights and deep shadows.")
                .show(ui);
            changed |= AdjustmentSlider::new("Dehaze", &mut exposure.dehaze, -100.0..=100.0)
                .decimals(0)
                .step(1.0)
                .hover_text(
                    "Removes or adds atmospheric veil while preserving color relationships.",
                )
                .show(ui);

            moduwu_design::section_separator(ui);
            ui.push_id("glow", |ui| {
                ui.strong("Glow");
                changed |= AdjustmentSlider::new("Amount", &mut exposure.glow_amount, 0.0..=100.0)
                    .decimals(0)
                    .step(1.0)
                    .hover_text(
                        "Softens and blooms bright light sources without lifting the entire image.",
                    )
                    .show(ui);
            });

            moduwu_design::section_separator(ui);
            changed |= AdjustmentSlider::new(
                "Halation",
                &mut exposure.halation_amount,
                0.0..=100.0,
            )
            .decimals(0)
            .step(1.0)
            .hover_text(
                "Adds a warm film halo around bright edges while preserving highlight cores.",
            )
            .show(ui);
            changed |= AdjustmentSlider::new("Grain", &mut exposure.grain_amount, 0.0..=100.0)
                .decimals(0)
                .step(1.0)
                .hover_text("Adds fine monochrome film grain, strongest in midtones.")
                .show(ui);

            moduwu_design::section_separator(ui);
            ui.push_id("vignette", |ui| {
                ui.strong("Vignette");
                changed |= AdjustmentSlider::new(
                    "Amount",
                    &mut exposure.vignette_amount,
                    -100.0..=100.0,
                )
                .decimals(0)
                .step(1.0)
                .hover_text(
                    "Darkens negative values or brightens positive values toward the image edges.",
                )
                .gradient(SliderGradient::Brightness)
                .show(ui);
                changed |= AdjustmentSlider::new(
                    "Midpoint",
                    &mut exposure.vignette_midpoint,
                    0.0..=100.0,
                )
                .decimals(0)
                .step(1.0)
                .hover_text(
                    "Moves the vignette transition inward or confines it to the outermost edge.",
                )
                .reset_to(ExposureParams::default().vignette_midpoint)
                .show(ui);
                changed |= AdjustmentSlider::new(
                    "Roundness",
                    &mut exposure.vignette_roundness,
                    -100.0..=100.0,
                )
                .decimals(0)
                .step(1.0)
                .hover_text("Changes the vignette shape from frame-like to circular.")
                .show(ui);
                changed |=
                    AdjustmentSlider::new("Feather", &mut exposure.vignette_feather, 0.0..=100.0)
                        .decimals(0)
                        .step(1.0)
                        .hover_text("Controls the softness of the vignette transition.")
                        .reset_to(ExposureParams::default().vignette_feather)
                        .show(ui);
                changed |= AdjustmentSlider::new(
                    "Highlights",
                    &mut exposure.vignette_highlights,
                    0.0..=100.0,
                )
                .decimals(0)
                .step(1.0)
                .hover_text("Restores bright edge highlights when using a dark vignette.")
                .gradient(SliderGradient::Brightness)
                .show(ui);
            });
        });
        changed |= action.apply(exposure, AdjustmentGroup::Effects);
        changed
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
