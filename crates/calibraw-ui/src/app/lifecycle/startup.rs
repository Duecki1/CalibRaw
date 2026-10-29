use super::*;
use crate::i18n::tr;

impl CalibRawApp {
    /// A desktop app with default settings and no GPU, for tests.
    #[cfg(test)]
    pub(crate) fn empty(ctx: &egui::Context) -> Self {
        Self::empty_on(
            ctx,
            crate::platform::fake::FakePlatform::new(PlatformCaps::DESKTOP),
        )
    }

    /// An app on `platform` with default settings and no GPU, for tests.
    #[cfg(test)]
    pub(crate) fn empty_on(ctx: &egui::Context, platform: Arc<dyn Platform>) -> Self {
        Self::from_performance_settings(
            ctx,
            None,
            crate::performance_settings::PerformanceSettings::default(),
            platform,
        )
    }

    pub(in crate::app) fn from_performance_settings(
        ctx: &egui::Context,
        performance_settings_path: Option<PathBuf>,
        performance: crate::performance_settings::PerformanceSettings,
        platform: Arc<dyn Platform>,
    ) -> Self {
        let caps = platform.caps();
        // HDD mode serializes reads from folders on rotational disks.
        if caps.has_folders() {
            calibraw_core::serialized_reads::set_enabled(performance.hdd_mode);
        }
        let discord_presence = DiscordPresence::new(performance.discord_rich_presence);
        let last_library_folder = performance.last_library_folder.clone();
        let last_library_selected_folder = performance.last_library_selected_folder.clone();
        // Folder storage reads profiles from a folder on disk, found in the
        // standard locations when asked to; a media library uses the copy
        // it imported, prewarmed before this.
        let (camera_profile_folder, camera_profile_folder_label) = if caps.has_folders() {
            let mut camera_profile_folder = performance.camera_profile_folder.clone();
            let mut camera_profile_folder_label = performance.camera_profile_folder_label.clone();
            if performance.camera_profile_auto_detect
                && camera_profile_folder
                    .as_ref()
                    .is_none_or(|folder| !folder.is_dir())
            {
                camera_profile_folder = crate::platform::detected_camera_profile_folder();
                if let Some(folder) = &camera_profile_folder {
                    calibraw_core::diagnostics::record(format!(
                        "DCP profiles: auto-detected folder {}",
                        folder.display()
                    ));
                    camera_profile_folder_label =
                        Some(tr("DCP profiles (auto-detected)").to_owned());
                } else {
                    camera_profile_folder_label = None;
                }
            }
            prewarm_dcp_profile_folder(camera_profile_folder.clone());
            (camera_profile_folder, camera_profile_folder_label)
        } else {
            (
                performance.camera_profile_folder.clone(),
                performance.camera_profile_folder_label.clone(),
            )
        };
        let exposure = ExposureParams::scene_referred_default();
        let geometry = GeometryTransform::default();
        let masks = MaskStack::default();
        let lens_correction = LensCorrectionState::default();
        let edit_history = EditHistory::new(&exposure, geometry, &masks, &lens_correction, None);
        let manual_runtime = if caps.configurable_ai_runtime {
            Self::load_onnx_runtime_selection(performance_settings_path.as_deref())
        } else {
            None
        };
        let library_preferences = crate::ui::library::LibraryPreferences {
            thumbnail_workers: performance.thumbnail_workers,
            automatic_thumbnail_workers: performance.thumbnail_workers_automatic,
            thumbnail_size: performance.library_thumbnail_size,
            sort_order: performance.library_sort_order,
            stack_raw_companions: performance.library_stack_raw_companions,
            render_edited_thumbnails_during_indexing: performance
                .render_edited_thumbnails_during_indexing,
        };
        let app = Self {
            develop: DevelopState {
                current_path: None,
                review: crate::sidecar::PhotoReview::default(),
                original_raw: None,
                loaded_raw: None,
                preview_raw: None,
                exposure,
                decoded_raws: DecodedRawCache::new(performance.raw_cache_files),
                neighbour_prefetch: Default::default(),
                selected_camera_profile: None,
                look_lut: None,
                look_lut_table: None,
                target_look: None,
                target_profile_color: None,
                profile_preview: Default::default(),
                geometry,
                geometry_revision: 0,
                lens_correction,
                target_exposure: exposure,
                lens_correction_dirty: false,
                load_receiver: None,
                loading_label: None,
                loading_view: None,
                image_status: tr("Open a RAW, TIFF, JPEG, PNG or HEIC photo to get started.")
                    .to_owned(),
                current_label: None,
            },
            preview: PreviewState {
                histogram: Default::default(),
                clipping: Default::default(),
                gpu_pipeline: None,
                program_template: None,
                retired_textures: TextureRetirement::new(ctx.clone()),
                gpu_prewarm_receiver: None,
                quality: performance.preview_quality,
                zoom: 1.0,
                center: [0.5, 0.5],
                visible_uv: PreviewUvRect::FULL,
                viewport_pixels: [1, 1],
                fitted_viewport_pixels: [1, 1],
                source_axes_swapped: false,
                motion_at: None,
                touch_navigation_active: false,
                space_pan_active: false,
                native_zoom: None,
                pending_view: None,
                revision: 0,
                detail: None,
                navigation: None,
                detail_pending_stage: None,
                navigation_pending_stage: None,
                detail_urgent: false,
                detail_block: None,
                quality_dirty: false,
                full_resolution: FullResolutionState::new(
                    performance.preview_full_resolution
                        && PlatformCaps::CURRENT.full_resolution_preview(),
                ),
                interactive: InteractiveState::default(),
                shrink_oversized: false,
                rebuild_receiver: None,
                detail_rebuild: None,
                original_exposure: exposure,
                original_requested: false,
                original_rendered_state: None,
                original_hold: None,
                pending_stage: None,
                dispatch_gate: PreviewDispatchGate::default(),
                lens_original_cache: None,
                lens_corrected_cache: None,
            },
            develop_ui: DevelopUiState {
                histogram_open: performance.develop_histogram_open,
                reference: DevelopReferenceState::default(),
                loading_thumbnail: DevelopLoadingThumbnailState::default(),
                filmstrip_open: performance.develop_filmstrip_open,
                filmstrip_centered: None,
                sidebar_open: true,
                crop_constraint_reference: None,
                crop_drag: None,
                crop_tool: CropTool::Crop,
                straighten_drag: None,
                perspective_guides: Vec::new(),
                guide_drag: None,
                area_picker: None,
                area_picker_drag: None,
                adjustment_section: AdjustmentSection::default(),
                reveal_lens_card: false,
                manual_lens_dialog: None,
                mask_section: MaskSection::default(),
                effect_component: None,
                mask_effect_component: None,
                mask_effect_mask: None,
                tone_curve_tab: ToneCurveTab::default(),
                color_grade_tab: ColorGradeTab::default(),
                hsl_mixer_color: HslMixerColor::default(),
                point_color: Default::default(),
                point_color_tab: false,
                mask_point_color: Default::default(),
                mask_point_color_tab: false,
                mask_point_color_mask: None,
            },
            masks: MaskState {
                stack: masks,
                active_tool: None,
                brush_mode: BrushMode::Paint,
                subject_refinement_active: false,
                drag: None,
                last_brush_point: None,
                touch_gesture_backup: None,
                interaction_dirty_layer: None,
                interaction_last_upload: None,
                interaction_has_uncommitted_change: false,
                overlay_revision: 0,
                overlay_texture: None,
                overlay_texture_key: None,
                overlay_blink: None,
                thumbnail_revision: 0,
                thumbnail_group_textures: Vec::new(),
                thumbnail_component_mask: None,
                thumbnail_component_textures: Vec::new(),
                source_cache: None,
                subject_cache: None,
                sky_cache: None,
                depth_cache: None,
                dirty_layers: [false; MAX_LOCAL_MASKS],
                detail_dirty_layers: [false; MAX_LOCAL_MASKS],
                navigation_dirty_layers: [false; MAX_LOCAL_MASKS],
            },
            ai: AiState {
                subject_mask_quality: performance.subject_mask_quality,
                people_mask_quality: performance.people_mask_quality,
                gpu_acceleration: performance.ai_gpu_acceleration,
                update_needed: false,
                update: None,
                runtime_mode: performance.onnx_runtime_mode,
                manual_runtime,
                library_mask_refresh: None,
                consent: None,
                object_pending_target: None,
                object_cache: None,
                people: PeopleState::default(),
                denoise_resume_pending: false,
            },
            inpaint: InpaintState {
                tool: InpaintTool::Remove,
                brush_size: 0.055,
                brush_hardness: 0.5,
                brush_opacity: 1.0,
                alignment: RetouchAlignment::None,
                source_point: None,
                source_pick_active: false,
                source_placement_press: false,
                aligned_offset: None,
                edits: Arc::new(RemoveEditState::default()),
                active_points: Vec::new(),
                last_brush_uv: None,
                pending_brush: None,
                pending_retouch: None,
                receiver: None,
                cancellation: None,
                processing_progress: None,
                hovered_stroke: None,
                selected_stroke: None,
                stroke_opacity_edit_pending: false,
            },
            export: ExportState {
                format: performance.export_format,
                gpu_prewarm: None,
                settings: ExportSettings::default(),
                task: None,
                batch: None,
                publish_pending: false,
                android_batch_load_pending: false,
                share_mime_type: None,
                override_choice: ExportOverrideChoice::default(),
                watermark: WatermarkState::new(performance.export_watermark),
            },
            persistence: PersistenceState {
                history: edit_history,
                lens_restore_masks: None,
                sidecar_target: None,
                document_generation: 0,
                sidecar_saved_revision: None,
                sidecar_failed_revision: None,
                sidecar_pending: VecDeque::new(),
                sidecar_in_flight: None,
                sidecar_receiver: None,
                sidecar_save_feedback_until: None,
                sidecar_save_error_dialog: None,
                sidecar_recovery: None,
                sidecar_autosave_deadline: None,
                developed_thumbnail_pending: None,
                developed_thumbnail_in_flight: None,
                developed_thumbnail_receiver: None,
            },
            usage: UsageState {
                app_persisted: Duration::from_millis(performance.app_usage_ms),
                app_started_at: Instant::now(),
                raw_accumulated: Duration::ZERO,
                raw_active_since: None,
            },
            presets: PresetState::load(
                performance_settings_path
                    .as_deref()
                    .and_then(crate::app::presets::preset_folder_for_settings),
            ),
            luts: LutState::open(
                performance_settings_path
                    .as_deref()
                    .map(crate::luts::lut_folder_for_settings),
            ),
            preferences: PreferencesState {
                image_relative_brush_size: performance.image_relative_brush_size,
                show_develop_navigation_labels: performance.show_develop_navigation_labels,
                develop_card_order: performance.develop_card_order.clone(),
                automatic_lens_correction: AutomaticLensCorrection {
                    enabled: performance.automatic_lens_correction,
                    corrections: LensfunCorrections {
                        geometry: performance.automatic_lens_geometry,
                        vignetting: performance.automatic_lens_vignetting,
                    },
                },
                missing_lens_profile_prompt: performance.missing_lens_profile_prompt,
                new_photo_preset: performance.new_photo_preset.clone(),
                export_name_template: performance.export_name_template.clone(),
                discord_rich_presence: performance.discord_rich_presence,
                hdd_mode: performance.hdd_mode,
                ui_design: performance.ui_design,
                preview_backdrop: performance.preview_backdrop,
                language: performance.language,
                onboarding_completed: performance.onboarding_completed,
                subject_mask_quality_chosen: performance.subject_mask_quality_chosen,
                tips: TipPreferences {
                    enabled: performance.tips_enabled,
                    seen: performance.seen_tips.clone(),
                },
                auto_check_updates: performance.auto_check_updates,
                github_update_check_allowed: performance.github_update_check_allowed,
                ignored_update_version: performance.ignored_update_version.clone(),
                adjustment_copy_settings: performance.adjustment_copy_settings,
                performance_settings_path,
                camera_profile_mode: performance.camera_profile_mode,
                camera_profile_folder,
                camera_profile_folder_label,
                camera_profile_auto_detect: performance.camera_profile_auto_detect,
                last_camera_profile: performance.last_camera_profile.clone(),
            },
            ui: UiState {
                thumbnail_cache_size: None,
                thumbnail_cache_size_receiver: None,
                active_tab: AppTab::default(),
                sidebar_tab: SidebarTab::default(),
                desktop_picker_receiver: None,
                status: tr("Open a RAW, TIFF, JPEG, PNG or HEIC photo to get started.").to_owned(),
                adaptive_preview_backdrop: crate::ui::theme::CANVAS_BACKDROP,
                notice: None,
                error_dialogs: ErrorDialogQueue::default(),
                unsupported_file_dialog: None,
                missing_lens_profile_prompt: None,
                runtime_restart_prompt: None,
                // Set below; the pending steps depend on the AI runtime state.
                onboarding_step: None,
                tip: None,
                version_check: Default::default(),
                shortcuts_dialog_open: false,
                settings_dialog: None,
                numeric_keyboard: false,
                review_feedback: None,
            },
            library: if caps.has_media_library() {
                LibraryState::new_android(
                    Arc::clone(&platform),
                    ctx,
                    library_preferences,
                    performance.last_android_library_folder,
                )
            } else {
                LibraryState::new_desktop(
                    Arc::clone(&platform),
                    library_preferences,
                    performance.library_folder_sidebar_open,
                )
            },
            discord_presence,
            toolbar_brand_texture: caps
                .is_desktop()
                .then(|| crate::ui::top_bar::load_toolbar_brand_texture(ctx)),
            egui_ctx: ctx.clone(),
            foreground_operation: None,
            android: AndroidState {
                picker_pending: false,
                pending_android_library_reset_reload: false,
                camera_profile_folder_importing_label: None,
                pending_android_profile_reload: None,
                photo_import: None,
            },
            platform,
        };
        let mut app = app;
        app.load_saved_watermark();
        app.ui.onboarding_step = OnboardingStep::pending(&app).first().copied();
        if caps.has_folders() {
            if let Some(folder) = last_library_folder.filter(|folder| folder.is_dir()) {
                app.library
                    .restore_folder(folder, last_library_selected_folder, ctx);
            }
        }
        app
    }

    /// Removes a file or folder an older version left in app data and nothing
    /// reads any more, on a background thread so start-up does not wait.
    ///
    /// migration: remove in v1.5.0, with the last of its callers. Listed in
    /// `calibraw_core`'s `migrations`.
    pub(in crate::app) fn remove_retired_app_data(path: PathBuf, description: &'static str) {
        let spawned = std::thread::Builder::new()
            .name("calibraw-retired-data-cleanup".to_owned())
            .spawn(move || {
                let removed = if path.is_dir() {
                    std::fs::remove_dir_all(&path)
                } else {
                    std::fs::remove_file(&path)
                };
                match removed {
                    Ok(()) => log::info!("removed {description} {}", path.display()),
                    Err(error) => log::warn!("could not remove {}: {error}", path.display()),
                }
            });
        if let Err(error) = spawned {
            log::warn!("could not start removing the {description}: {error}");
        }
    }

    /// The editor on `platform`, started from its saved settings.
    pub(crate) fn new(cc: &eframe::CreationContext<'_>, platform: Arc<dyn Platform>) -> Self {
        let caps = platform.caps();
        let mut editor_gpu = None;
        if let Some(render_state) = cc.wgpu_render_state.as_ref() {
            calibraw_gpu::install_uncaptured_gpu_error_handler(&render_state.device);
            calibraw_gpu::stage_timing::install(&render_state.device, &render_state.queue);
            // A lost device is recovered by restarting (`handle_gpu_device_loss`).
            if caps.can_restart {
                calibraw_gpu::install_gpu_device_lost_handler(&render_state.device);
            }
            editor_gpu = Some(calibraw_ai::GpuIdentity::new(&render_state.device));
        }
        let repaint = cc.egui_ctx.clone();
        platform.attach_ui(Box::new(move || repaint.request_repaint()));
        crate::ui::theme::install(&cc.egui_ctx);
        match platform.device_diagnostics() {
            Ok(Some(info)) => calibraw_core::diagnostics::set_device_info(info),
            Ok(None) => {}
            Err(error) => calibraw_core::diagnostics::record(error),
        }
        calibraw_core::diagnostics::record(if caps.is_mobile() {
            tr("CalibRaw Android UI initialized")
        } else {
            tr("CalibRaw desktop UI initialized")
        });
        let performance_settings_path = platform
            .performance_settings_path()
            .map_err(|error| log::warn!("{error}"))
            .ok()
            .flatten();
        let gpu_pipeline_cache_root = platform
            .gpu_pipeline_cache_dir()
            .map_err(|error| log::warn!("{error}"))
            .ok()
            .flatten();
        if std::env::var_os("CALIBRAW_LENSFUN_DB").is_none() {
            match platform.lensfun_database_dir() {
                Ok(Some(path)) => {
                    // Android copied Lensfun 0.3.4's database into `version_1`;
                    // the bundled `version_2` beside it replaces it.
                    // migration: remove in v1.5.0. Listed in `calibraw_core`'s
                    // `migrations`.
                    let retired = path.join("version_1");
                    if retired.is_dir() {
                        Self::remove_retired_app_data(retired, tr("Lensfun 0.3.4 database"));
                    }
                    std::env::set_var("CALIBRAW_LENSFUN_DB", &path);
                    calibraw_core::diagnostics::record(format!(
                        "bundled Lensfun database materialized at {}",
                        path.display()
                    ));
                }
                Ok(None) => {}
                Err(error) => log::warn!("{error}"),
            }
        }
        // Before the app reads the ONNX Runtime selection or looks for models.
        let moved_ai_file_failures: Vec<String> = if caps.is_desktop() {
            [
                Self::move_onnx_runtime_selection(performance_settings_path.as_deref()),
                Self::move_desktop_model_folder(
                    performance_settings_path.as_deref(),
                    &platform.model_root(),
                ),
            ]
            .into_iter()
            .flatten()
            .collect()
        } else {
            Vec::new()
        };
        let performance = crate::performance_settings::load(performance_settings_path.as_deref());
        // A media library uses imported copies of profile folders; drop the
        // ones no setting names any more.
        if caps.has_media_library() {
            if let Err(error) = platform
                .scavenge_camera_profile_mirrors(performance.camera_profile_folder.as_deref())
            {
                log::warn!("{error}");
            }
            prewarm_dcp_profile_folder(performance.camera_profile_folder.clone());
        }
        let gpu_export_prewarm = Arc::new(crate::pipeline::GpuProgramPrewarm::new());
        let gpu_preview_prewarm_receiver = spawn_gpu_preview_prewarm(
            cc,
            gpu_pipeline_cache_root,
            Arc::clone(&gpu_export_prewarm),
            Arc::clone(&platform),
        );
        let mut app = Self::from_performance_settings(
            &cc.egui_ctx,
            performance_settings_path,
            performance,
            platform,
        );
        app.preview.gpu_prewarm_receiver = gpu_preview_prewarm_receiver;
        app.export.gpu_prewarm = Some(gpu_export_prewarm);
        crate::ui::theme::apply(&cc.egui_ctx, app.preferences.ui_design);
        if let Err(error) = crate::i18n::set_active(app.preferences.language.resolve()) {
            app.report_error(ErrorKind::Settings, error.to_string());
        }
        if let Err(error) = app
            .platform
            .set_light_system_bars(!app.preferences.ui_design.is_dark())
        {
            log::warn!("{error}");
        }
        for failure in moved_ai_file_failures {
            app.report_error(ErrorKind::AiFilesNotMoved, failure);
        }
        app.remove_legacy_ai_denoise_cache();
        if caps.is_desktop() {
            app.remove_retired_depth_model();
            app.remove_retired_onnx_runtimes();
        }
        // Where the runtime is built in (Android), GPU providers are always
        // allowed and no GPU job record is kept.
        let interrupted_gpu_ai = calibraw_ai::configure(calibraw_ai::AiConfig {
            acceleration: !caps.configurable_ai_runtime || app.ai.gpu_acceleration,
            runtime: app.ai_runtime_source(),
            model_root: app.ai_model_root(),
            job_record_path: app.platform.gpu_ai_job_record_path(),
            gpu: editor_gpu,
        });
        if let Some(activity) = interrupted_gpu_ai {
            app.handle_interrupted_gpu_ai(&activity);
        }
        app
    }
}
