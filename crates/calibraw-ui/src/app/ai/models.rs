use super::*;
use crate::i18n::{tr, tr_fmt};

impl CalibRawApp {
    pub(crate) fn set_onnx_runtime_mode(&mut self, mode: OnnxRuntimeMode) {
        if self.ai.runtime_mode == mode {
            return;
        }
        self.ai.runtime_mode = mode;
        self.sync_onnx_runtime_source();
        self.ai.consent = None;
        self.persist_performance_settings();
        self.ui.notice = Some(
            tr("ONNX Runtime mode changed. Restart CalibRaw if an AI runtime was already used in this session.")
                .to_owned(),
        );
        self.prompt_restart_for_runtime_change(tr("ONNX Runtime mode changed."));
    }

    /// Asks for a restart when this process already loaded an ONNX Runtime
    /// library: the new choice cannot replace that library until CalibRaw
    /// starts again, and AI jobs would fail until then.
    pub(in crate::app) fn prompt_restart_for_runtime_change(&mut self, change: &str) {
        if calibraw_ai::runtime::runtime_loaded_in_process() {
            self.ui.runtime_restart_prompt = Some(tr_fmt("{change} The ONNX Runtime already loaded in this session cannot be replaced while CalibRaw runs. Restart CalibRaw before using AI features again.", &[("change", &change)]));
        }
    }

    pub(in crate::app) fn show_runtime_restart_dialog(&mut self, ctx: &egui::Context) {
        let Some(message) = self.ui.runtime_restart_prompt.clone() else {
            return;
        };
        let mut close = false;
        moduwu_design::Dialog::new("onnx-runtime-restart", "Restart required").show_with_footer(
            ctx,
            |ui| {
                ui.label(&message);
            },
            |ui| {
                close |= moduwu_design::primary_action_button(ui, tr("OK")).clicked();
                if !close
                    && moduwu_design::dialog_keyboard_action(
                        ui,
                        moduwu_design::DialogKeyboard::CLOSE_ONLY,
                        false,
                    ) == moduwu_design::DialogAction::Cancel
                {
                    close = true;
                }
            },
        );
        if close {
            self.ui.runtime_restart_prompt = None;
        }
    }

    pub(in crate::app) fn automatic_onnx_runtime_download_needed(&self) -> bool {
        self.caps().configurable_ai_runtime
            && self.ai.runtime_mode == OnnxRuntimeMode::Automatic
            && calibraw_ai::runtime::bundled_onnx_runtime_path().is_none()
            && !calibraw_ai::runtime::automatic_onnx_runtime_is_installed()
    }

    pub(in crate::app) fn show_automatic_onnx_runtime_download_details(ui: &mut egui::Ui) {
        ui.separator();
        ui.strong(tr("ONNX Runtime"));
        let runtime = calibraw_ai::runtime::automatic_onnx_runtime_info();
        if let Some(runtime) = runtime {
            ui.label(tr_fmt(
                "ONNX Runtime {version} for {platform}: {size} MB download.",
                &[
                    ("version", &runtime.version),
                    ("platform", &runtime.platform),
                    (
                        "size",
                        &format!("{:.1}", runtime.download_bytes as f64 / 1_000_000.0),
                    ),
                ],
            ));
        } else {
            ui.label(tr_fmt(
                "Automatic ONNX Runtime for {os} / {arch}.",
                &[
                    ("os", &std::env::consts::OS),
                    ("arch", &std::env::consts::ARCH),
                ],
            ));
        }
        ui.label(tr("SHA-256 verified and cached locally (MIT license)."));
        if let Some(url) = runtime.and_then(|runtime| runtime.download_url) {
            ui.hyperlink_to(tr("Runtime source"), url);
        }
    }

    /// Runtime details shared by every local-AI consent dialog.
    pub(in crate::app) fn show_ai_consent_runtime_details(
        &self,
        ui: &mut egui::Ui,
        runtime_download_needed: bool,
    ) {
        if runtime_download_needed {
            Self::show_automatic_onnx_runtime_download_details(ui);
        }
    }

    pub(in crate::app) fn show_ai_download_summary(
        ui: &mut egui::Ui,
        model_name: &str,
        task: &str,
        model_download_needed: bool,
        runtime_download_needed: bool,
    ) {
        let download = match (model_download_needed, runtime_download_needed) {
            (true, true) => tr_fmt(
                "{model_name} and ONNX Runtime",
                &[("model_name", &model_name)],
            ),
            (true, false) => model_name.to_owned(),
            (false, true) => tr("ONNX Runtime").to_owned(),
            (false, false) => String::new(),
        };
        if download.is_empty() {
            ui.label(tr_fmt("CalibRaw will {task} locally.", &[("task", &task)]));
        } else {
            ui.label(tr_fmt(
                "CalibRaw will download {download} to {task} locally.",
                &[("download", &download), ("task", &task)],
            ));
            ui.label(tr(
                "Your photo stays on this device; Hugging Face receives your IP address.",
            ));
        }
    }

    pub(in crate::app) fn show_ai_download_details(
        &self,
        ui: &mut egui::Ui,
        id: &'static str,
        model_download_needed: bool,
        runtime_download_needed: bool,
        license_links: &[(&str, &str)],
        model_details: impl FnOnce(&mut egui::Ui),
    ) {
        if !model_download_needed && !runtime_download_needed {
            return;
        }
        egui::CollapsingHeader::new(tr("Model, runtime and privacy details"))
            .id_salt(id)
            .show(ui, |ui| {
                if model_download_needed {
                    model_details(ui);
                }
                self.show_ai_consent_runtime_details(ui, runtime_download_needed);
                Self::show_hugging_face_privacy(
                    ui,
                    if model_download_needed {
                        license_links
                    } else {
                        &[]
                    },
                );
            });
    }

    /// Warning shown when Manual runtime mode has no trusted library. Every local-AI consent
    /// dialog has to say the same thing here.
    pub(in crate::app) fn show_manual_runtime_warning(&self, ui: &mut egui::Ui) {
        if self.manual_onnx_runtime() && self.ai.manual_runtime.is_none() {
            ui.colored_label(
                egui::Color32::YELLOW,
                tr("Manual runtime mode needs a trusted local ONNX Runtime library. Select one in Settings or switch to Automatic."),
            );
        }
    }

    /// The decline and accept buttons of the local-AI consent dialog; accept is
    /// enabled only while the runtime is usable.
    pub(in crate::app) fn show_ai_consent_buttons(
        ui: &mut egui::Ui,
        decline_label: &str,
        accept_label: &str,
        accept_enabled: bool,
    ) -> moduwu_design::DialogAction {
        moduwu_design::dialog_confirmation_actions(
            ui,
            decline_label,
            accept_label,
            accept_enabled,
            false,
            moduwu_design::DialogKeyboard::CLOSE_ONLY,
        )
    }

    /// Privacy statement and links shared by all four local-AI consent dialogs.
    pub(in crate::app) fn show_hugging_face_privacy(
        ui: &mut egui::Ui,
        license_links: &[(&str, &str)],
    ) {
        ui.label(tr("Hugging Face receives your IP address and request time; CalibRaw sends no photo, account ID, or telemetry."));
        ui.horizontal_wrapped(|ui| {
            ui.hyperlink_to(
                tr("Hugging Face privacy policy"),
                "https://huggingface.co/privacy",
            );
            for &(label, url) in license_links {
                ui.separator();
                ui.hyperlink_to(label, url);
            }
        });
    }

    pub(in crate::app) fn ai_model_root(&self) -> PathBuf {
        self.platform.model_root()
    }

    pub(in crate::app) fn sam21_model_paths(&self) -> (PathBuf, PathBuf) {
        let root = self.ai_model_root();
        (
            root.join("sam2.1-hiera-tiny.encoder.onnx"),
            root.join("sam2.1-hiera-tiny.decoder.onnx"),
        )
    }

    /// What people masks made now are made from: the people-mask quality on
    /// this device.
    pub(crate) fn people_segmentation(&self) -> calibraw_ai::models::people::PeopleSegmentation {
        self.ai.people_mask_quality.segmentation()
    }

    /// The models people masks made now need.
    pub(in crate::app) fn people_models(&self) -> calibraw_ai::models::people::PeopleModels {
        calibraw_ai::models::people::PeopleModels::new(
            self.people_segmentation(),
            &self.ai_model_root(),
            self.subject_model_precision(),
        )
    }

    /// Float16 when subject masks are expected to run on the GPU; see
    /// `SubjectModelPrecision::for_current_sessions`.
    pub(in crate::app) fn subject_model_precision(
        &self,
    ) -> calibraw_ai::models::subject::SubjectModelPrecision {
        calibraw_ai::models::subject::SubjectModelPrecision::for_current_sessions(
            self.manual_onnx_runtime(),
        )
    }

    /// A manually selected ONNX Runtime, whose execution providers are not
    /// known before it loads.
    pub(in crate::app) fn manual_onnx_runtime(&self) -> bool {
        self.caps().configurable_ai_runtime && self.ai.runtime_mode == OnnxRuntimeMode::Manual
    }

    pub(in crate::app) fn subject_model(&self) -> calibraw_ai::ModelInfo {
        self.ai
            .subject_mask_quality
            .model(self.subject_model_precision())
    }

    pub(in crate::app) fn birefnet_model_path(
        &self,
        precision: calibraw_ai::models::subject::SubjectModelPrecision,
    ) -> PathBuf {
        self.ai_model_root()
            .join(self.ai.subject_mask_quality.model(precision).cache_filename)
    }

    pub(in crate::app) fn skyseg_model_path(&self) -> PathBuf {
        self.ai_model_root()
            .join(calibraw_ai::models::sky::MODEL.cache_filename)
    }

    pub(in crate::app) fn depth_model_path(&self) -> PathBuf {
        self.ai_model_root()
            .join(calibraw_ai::models::depth::MODEL.cache_filename)
    }

    /// Desktop builds used a larger depth model before Depth Anything V2
    /// Small; nothing reads it any more. Removes that file and nothing else.
    ///
    /// migration: remove in v1.4.0. Listed in `calibraw_core`'s `migrations`.
    pub(in crate::app) fn remove_retired_depth_model(&self) {
        let path = self
            .ai_model_root()
            .join(calibraw_ai::models::depth::RETIRED_DEPTH_MODEL_FILENAME);
        if path.is_file() {
            Self::remove_retired_app_data(path, "retired depth model");
        }
    }

    /// Automatic runtimes the WebGPU packages replaced: the CPU ones, the
    /// Linux CUDA ones and the Windows DirectML ones. Removes their install
    /// folders and archives in the model cache and nothing else, keeping a
    /// folder that holds the manually selected runtime.
    ///
    /// migration: remove in v1.5.0. Listed in `calibraw_core`'s `migrations`.
    pub(in crate::app) fn remove_retired_onnx_runtimes(&self) {
        let root = self.ai_model_root();
        let manual = self.ai.manual_runtime.as_ref().map(|(path, _)| path);
        for path in calibraw_ai::runtime::retired_runtime_paths(&root) {
            if path.exists() && !manual.is_some_and(|manual| manual.starts_with(&path)) {
                Self::remove_retired_app_data(path, "retired ONNX Runtime");
            }
        }
    }

    pub(in crate::app) fn big_lama_model_path(&self) -> PathBuf {
        self.ai_model_root()
            .join(calibraw_ai::models::remove::MODEL.cache_filename)
    }

    /// The file naming the manually selected ONNX Runtime, beside the
    /// settings file at `settings_path` as the presets are.
    fn onnx_runtime_selection_path(settings_path: Option<&std::path::Path>) -> Option<PathBuf> {
        settings_path?
            .parent()
            .map(|folder| folder.join("onnx-runtime-path"))
    }

    /// Moves the ONNX Runtime selection that versions up to 1.3 saved in the
    /// config or temporary folder beside the settings file, before it is
    /// loaded. Returns the failure to report.
    ///
    /// migration: remove in v2.0.0, with
    /// `calibraw_core::migrations::onnx_runtime_selection`.
    pub(in crate::app) fn move_onnx_runtime_selection(
        settings_path: Option<&std::path::Path>,
    ) -> Option<String> {
        use calibraw_core::migrations::onnx_runtime_selection;
        let selection = Self::onnx_runtime_selection_path(settings_path)?;
        let legacy = onnx_runtime_selection::legacy_onnx_runtime_selection_path();
        onnx_runtime_selection::move_onnx_runtime_selection(&legacy, &selection)
            .err()
            .map(|error| {
                tr_fmt("CalibRaw could not move the ONNX Runtime selection from {legacy} to {selection} ({error}). Select the ONNX Runtime library again in Settings.", &[("legacy", &legacy.display()), ("selection", &selection.display()), ("error", &error)])
            })
    }

    /// Moves the models and runtimes that desktop versions up to 1.3
    /// downloaded into a cache or the temporary folder to `model_root`,
    /// before anything looks for them. A manually selected runtime in the old
    /// folder stays there, since the settings name it by its path. Returns
    /// the failure to report.
    ///
    /// migration: remove in v1.5.0, with
    /// `calibraw_core::migrations::desktop_model_folder`.
    pub(in crate::app) fn move_desktop_model_folder(
        settings_path: Option<&std::path::Path>,
        model_root: &std::path::Path,
    ) -> Option<String> {
        use calibraw_core::migrations::desktop_model_folder;
        let legacy = desktop_model_folder::legacy_desktop_model_root();
        let manual_runtime = Self::load_onnx_runtime_selection(settings_path).map(|(path, _)| path);
        desktop_model_folder::move_desktop_models(&legacy, model_root, manual_runtime.as_deref())
            .err()
            .map(|error| {
                tr_fmt("CalibRaw could not move the downloaded AI models from {legacy} to {model_root} ({error}). Models left behind are downloaded again when needed, and the old folder can then be deleted.", &[("legacy", &legacy.display()), ("model_root", &model_root.display()), ("error", &error)])
            })
    }

    pub(in crate::app) fn load_onnx_runtime_selection(
        settings_path: Option<&std::path::Path>,
    ) -> Option<(PathBuf, calibraw_ai::Sha256)> {
        let configured =
            std::fs::read_to_string(Self::onnx_runtime_selection_path(settings_path)?).ok()?;
        let mut lines = configured.lines();
        let sha256 = calibraw_ai::Sha256::parse(lines.next()?.strip_prefix("sha256=")?).ok()?;
        let path = PathBuf::from(lines.next()?.strip_prefix("path=")?);
        if lines.next().is_some() || !path.is_file() {
            return None;
        }
        Some((path, sha256))
    }

    pub(in crate::app) fn persist_onnx_runtime_selection(
        settings_path: Option<&std::path::Path>,
        selection: Option<(&std::path::Path, &calibraw_ai::Sha256)>,
    ) -> Result<(), String> {
        let config = match Self::onnx_runtime_selection_path(settings_path) {
            Some(config) => config,
            None if selection.is_none() => return Ok(()),
            None => {
                return Err(
                    tr("there is no settings folder to save the ONNX Runtime selection in")
                        .to_owned(),
                )
            }
        };
        if let Some((path, sha256)) = selection {
            let path_text = path
                .to_str()
                .ok_or_else(|| tr("the ONNX Runtime path is not valid UTF-8").to_owned())?;
            if path_text.contains('\n') || path_text.contains('\r') {
                return Err(tr("the ONNX Runtime path contains a line break").to_owned());
            }
            let payload = format!("sha256={sha256}\npath={path_text}\n");
            calibraw_core::file_ops::write_bytes_atomically(&config, payload.as_bytes()).map_err(
                |error| {
                    tr_fmt(
                        "could not save {config}: {error}",
                        &[("config", &config.display()), ("error", &error)],
                    )
                },
            )?;
        } else {
            match std::fs::remove_file(&config) {
                Ok(()) => {
                    if let Some(parent) = config.parent() {
                        calibraw_core::file_ops::sync_parent_directory(parent).map_err(
                            |error| {
                                tr_fmt(
                                    "could not flush {parent}: {error}",
                                    &[("parent", &parent.display()), ("error", &error)],
                                )
                            },
                        )?;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(tr_fmt(
                        "could not remove {config}: {error}",
                        &[("config", &config.display()), ("error", &error)],
                    ));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn choose_onnx_runtime(&mut self) {
        if self.ui.desktop_picker_receiver.is_some() {
            return;
        }
        let mut dialog =
            crate::platform::FileDialog::new().set_title("Select the ONNX Runtime shared library");
        if let Some(parent) = self
            .ai
            .manual_runtime
            .as_ref()
            .and_then(|(path, _)| path.parent())
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            dialog = dialog.set_directory(parent);
        }
        let settings_path = self.preferences.performance_settings_path.clone();
        self.ui.desktop_picker_receiver = Some(spawn_ui_worker(&self.egui_ctx, move || {
            let result = dialog
                .pick_file_on_worker()
                .map(|path| Self::validate_and_persist_onnx_runtime(path, settings_path.as_deref()))
                .transpose();
            crate::app::DesktopPickerEvent::OnnxRuntime(result)
        }));
    }

    pub(in crate::app) fn validate_and_persist_onnx_runtime(
        path: PathBuf,
        settings_path: Option<&std::path::Path>,
    ) -> Result<(PathBuf, calibraw_ai::Sha256), String> {
        if !path.is_file() {
            return Err(tr_fmt(
                "{path} is not a file.",
                &[("path", &path.display())],
            ));
        }
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !crate::platform::is_onnx_runtime_library_name(&file_name) {
            return Err(
                tr("Select the ONNX Runtime shared library (onnxruntime.dll, libonnxruntime.so, or libonnxruntime.dylib).")
                    .to_owned(),
            );
        }
        let sha256 = calibraw_ai::Sha256::of_file(&path).map_err(|error| {
            tr_fmt(
                "Could not hash selected ONNX Runtime: {error}",
                &[("error", &format!("{error:#}"))],
            )
        })?;
        if let Err(error) = calibraw_ai::runtime::probe_runtime_subprocess(&path, &sha256) {
            return Err(tr_fmt(
                "This ONNX Runtime could not be loaded safely: {error}",
                &[("error", &format!("{error:#}"))],
            ));
        }
        Self::persist_onnx_runtime_selection(settings_path, Some((&path, &sha256)))?;
        Ok((path, sha256))
    }

    pub(crate) fn clear_onnx_runtime(&mut self) {
        match Self::persist_onnx_runtime_selection(
            self.preferences.performance_settings_path.as_deref(),
            None,
        ) {
            Ok(()) => {
                self.ai.manual_runtime = None;
                self.sync_onnx_runtime_source();
                self.ui.notice = Some(
                    tr("Manual ONNX Runtime selection cleared. Choose another library or switch to Automatic. Restart CalibRaw to apply the change.")
                        .to_owned(),
                );
                self.prompt_restart_for_runtime_change(tr(
                    "Manual ONNX Runtime selection cleared.",
                ));
            }
            Err(error) => self.report_error(ErrorKind::AiRuntime, error),
        }
    }
}

/// The ONNX Runtime choice, offered where `configurable_ai_runtime`.
impl CalibRawApp {
    /// The runtime AI jobs load. Manual mode without a selected library
    /// falls back to Automatic; `validate_onnx_runtime_for_ai` stops jobs
    /// that ask for it. Always Automatic where the runtime is built in.
    pub(in crate::app) fn ai_runtime_source(&self) -> calibraw_ai::RuntimeSource {
        if !self.caps().configurable_ai_runtime {
            return calibraw_ai::RuntimeSource::Automatic;
        }
        match (self.ai.runtime_mode, &self.ai.manual_runtime) {
            (OnnxRuntimeMode::Manual, Some((path, sha256))) => calibraw_ai::RuntimeSource::Manual {
                path: path.clone(),
                sha256: sha256.clone(),
            },
            (OnnxRuntimeMode::Automatic, _) | (OnnxRuntimeMode::Manual, None) => {
                calibraw_ai::RuntimeSource::Automatic
            }
        }
    }

    /// Hands the runtime the current mode and selection call for to
    /// `calibraw_ai`.
    pub(in crate::app) fn sync_onnx_runtime_source(&self) {
        calibraw_ai::update(calibraw_ai::AiConfigChange::Runtime(
            self.ai_runtime_source(),
        ));
    }
}
