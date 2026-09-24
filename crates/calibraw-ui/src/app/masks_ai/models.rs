use super::*;

impl CalibRawApp {
    #[cfg(not(target_os = "android"))]
    pub(crate) fn set_onnx_runtime_mode(&mut self, mode: OnnxRuntimeMode) {
        if self.ai.runtime_mode == mode {
            return;
        }
        self.ai.runtime_mode = mode;
        self.ai.consent = AiConsentState::None;
        self.persist_performance_settings();
        self.ui.notice = Some(
            "ONNX Runtime mode changed. Restart CalibRaw if an AI runtime was already used in this session."
                .to_owned(),
        );
    }

    #[cfg(not(target_os = "android"))]
    pub(in crate::app) fn onnx_runtime_for_ai(&self) -> (Option<PathBuf>, Option<String>) {
        match self.ai.runtime_mode {
            OnnxRuntimeMode::Automatic => (None, None),
            OnnxRuntimeMode::Manual => {
                (self.ai.runtime_path.clone(), self.ai.runtime_sha256.clone())
            }
        }
    }

    pub(in crate::app) fn automatic_onnx_runtime_download_needed(&self) -> bool {
        #[cfg(not(target_os = "android"))]
        {
            self.ai.runtime_mode == OnnxRuntimeMode::Automatic
                && !calibraw_ai::automatic_onnx_runtime_is_installed()
        }
        #[cfg(target_os = "android")]
        {
            false
        }
    }

    #[cfg(not(target_os = "android"))]
    pub(in crate::app) fn show_automatic_onnx_runtime_download_details(ui: &mut egui::Ui) {
        ui.separator();
        ui.strong("ONNX Runtime");
        if let Some(runtime) = calibraw_ai::automatic_onnx_runtime_info() {
            ui.label(format!(
                "ONNX Runtime {} for {}: {:.1} MB download.",
                runtime.version,
                runtime.platform,
                runtime.download_bytes as f64 / 1_000_000.0
            ));
        } else {
            ui.label(format!(
                "Automatic ONNX Runtime for {} / {}.",
                std::env::consts::OS,
                std::env::consts::ARCH
            ));
        }
        ui.label("Downloaded from CalibRaw Artifacts, SHA-256 verified, and cached locally (MIT license).");
        ui.hyperlink_to(
            "Runtime source",
            "https://huggingface.co/Duecki/CalibRaw-Artifacts/tree/main/onnxruntime",
        );
    }

    /// Runtime details shared by every local-AI consent dialog.
    pub(in crate::app) fn show_ai_consent_runtime_details(
        &self,
        ui: &mut egui::Ui,
        runtime_download_needed: bool,
    ) {
        #[cfg(not(target_os = "android"))]
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
            (true, true) => format!("{model_name} and ONNX Runtime"),
            (true, false) => model_name.to_owned(),
            (false, true) => "ONNX Runtime".to_owned(),
            (false, false) => String::new(),
        };
        if download.is_empty() {
            ui.label(format!("CalibRaw will {task} locally."));
        } else {
            ui.label(format!(
                "CalibRaw will download {download} to {task} locally."
            ));
            ui.label("Your photo stays on this device; Hugging Face receives your IP address.");
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
        egui::CollapsingHeader::new("Model, runtime and privacy details")
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
        #[cfg(not(target_os = "android"))]
        if self.ai.runtime_mode == OnnxRuntimeMode::Manual && self.ai.runtime_path.is_none() {
            ui.colored_label(
                egui::Color32::YELLOW,
                "Manual runtime mode needs a trusted local ONNX Runtime library. Select one in Settings or switch to Automatic.",
            );
        }
    }

    /// The consent row shared by every local-AI download dialog: Cancel, plus an accept button
    /// whose enablement the caller decides — AI Denoise gates on the model download, the mask and
    /// Remove dialogs gate on the runtime being usable.
    pub(in crate::app) fn show_ai_consent_buttons(
        ui: &mut egui::Ui,
        accept_label: &str,
        accept_enabled: bool,
    ) -> crate::ui::theme::DialogAction {
        crate::ui::theme::dialog_confirmation_buttons(
            ui,
            "Cancel",
            accept_label,
            accept_enabled,
            false,
            crate::ui::theme::DialogKeyboard::CLOSE_ONLY,
        )
    }

    /// Privacy statement and links shared by all four local-AI consent dialogs.
    pub(in crate::app) fn show_hugging_face_privacy(
        ui: &mut egui::Ui,
        license_links: &[(&str, &str)],
    ) {
        ui.label("Hugging Face receives your IP address and request time; CalibRaw sends no photo, account ID, or telemetry.");
        ui.horizontal_wrapped(|ui| {
            ui.hyperlink_to(
                "Hugging Face privacy policy",
                "https://huggingface.co/privacy",
            );
            for &(label, url) in license_links {
                ui.separator();
                ui.hyperlink_to(label, url);
            }
        });
    }

    pub(in crate::app) fn ai_model_root(&self) -> PathBuf {
        #[cfg(not(target_os = "android"))]
        {
            calibraw_ai::desktop_model_cache_root()
        }
        #[cfg(target_os = "android")]
        {
            self.android
                .android_app
                .internal_data_path()
                .unwrap_or_else(std::env::temp_dir)
                .join("models")
        }
    }

    pub(in crate::app) fn sam21_model_paths(&self) -> (PathBuf, PathBuf) {
        let root = self.ai_model_root();
        (
            root.join("sam2.1-hiera-tiny.encoder.onnx"),
            root.join("sam2.1-hiera-tiny.decoder.onnx"),
        )
    }

    pub(in crate::app) fn birefnet_model_path(&self) -> PathBuf {
        self.ai_model_root()
            .join(self.ai.birefnet_quality.model().cache_filename)
    }

    pub(in crate::app) fn big_lama_model_path(&self) -> PathBuf {
        self.ai_model_root()
            .join(crate::remove::BIG_LAMA_MODEL_FILENAME)
    }

    #[cfg(not(target_os = "android"))]
    pub(in crate::app) fn onnx_runtime_config_path() -> PathBuf {
        let root = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .unwrap_or_else(std::env::temp_dir);
        root.join("calibraw/onnx-runtime-path")
    }

    #[cfg(not(target_os = "android"))]
    pub(in crate::app) fn load_onnx_runtime_selection() -> Option<(PathBuf, String)> {
        let configured = std::fs::read_to_string(Self::onnx_runtime_config_path()).ok()?;
        let mut lines = configured.lines();
        let sha256 = lines.next()?.strip_prefix("sha256=")?.to_owned();
        let path = PathBuf::from(lines.next()?.strip_prefix("path=")?);
        if lines.next().is_some()
            || sha256.len() != 64
            || !sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || !path.is_file()
        {
            return None;
        }
        Some((path, sha256))
    }

    #[cfg(not(target_os = "android"))]
    pub(in crate::app) fn persist_onnx_runtime_selection(
        selection: Option<(&std::path::Path, &str)>,
    ) -> Result<(), String> {
        let config = Self::onnx_runtime_config_path();
        if let Some((path, sha256)) = selection {
            let parent = config
                .parent()
                .ok_or_else(|| "invalid CalibRaw configuration path".to_owned())?;
            let path_text = path
                .to_str()
                .ok_or_else(|| "the ONNX Runtime path is not valid UTF-8".to_owned())?;
            if path_text.contains('\n') || path_text.contains('\r') {
                return Err("the ONNX Runtime path contains a line break".to_owned());
            }
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
            let temporary = config.with_extension(format!("tmp.{}", std::process::id()));
            let payload = format!("sha256={sha256}\npath={path_text}\n");
            let result = (|| {
                use std::io::Write as _;

                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .truncate(true)
                    .write(true)
                    .open(&temporary)
                    .map_err(|error| format!("could not open {}: {error}", temporary.display()))?;
                file.write_all(payload.as_bytes())
                    .map_err(|error| format!("could not write {}: {error}", temporary.display()))?;
                file.sync_all()
                    .map_err(|error| format!("could not flush {}: {error}", temporary.display()))?;
                drop(file);
                crate::file_ops::replace_file(&temporary, &config)
                    .map_err(|error| format!("could not publish {}: {error}", config.display()))?;
                crate::file_ops::sync_parent_directory(parent)
                    .map_err(|error| format!("could not flush {}: {error}", parent.display()))
            })();
            if result.is_err() {
                let _ = std::fs::remove_file(&temporary);
            }
            result?;
        } else {
            match std::fs::remove_file(&config) {
                Ok(()) => {
                    if let Some(parent) = config.parent() {
                        crate::file_ops::sync_parent_directory(parent).map_err(|error| {
                            format!("could not flush {}: {error}", parent.display())
                        })?;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!("could not remove {}: {error}", config.display()));
                }
            }
        }
        Ok(())
    }

    #[cfg(not(target_os = "android"))]
    pub(crate) fn choose_onnx_runtime(&mut self) {
        if self.ui.desktop_picker_receiver.is_some() {
            return;
        }
        let mut dialog =
            rfd::AsyncFileDialog::new().set_title("Select the ONNX Runtime shared library");
        if let Some(parent) = self
            .ai
            .runtime_path
            .as_deref()
            .and_then(|path| path.parent())
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            dialog = dialog.set_directory(parent);
        }
        self.ui.desktop_picker_receiver = Some(spawn_ui_worker(&self.egui_ctx, move || {
            let result = pollster::block_on(dialog.pick_file())
                .map(|handle| handle.path().to_path_buf())
                .map(Self::validate_and_persist_onnx_runtime)
                .transpose();
            crate::app::DesktopPickerEvent::OnnxRuntime(result)
        }));
    }

    #[cfg(not(target_os = "android"))]
    pub(in crate::app) fn validate_and_persist_onnx_runtime(
        path: PathBuf,
    ) -> Result<(PathBuf, String), String> {
        if !path.is_file() {
            return Err(format!("{} is not a file.", path.display()));
        }
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let looks_like_runtime = if cfg!(target_os = "windows") {
            file_name == "onnxruntime.dll"
        } else if cfg!(target_os = "macos") {
            file_name == "libonnxruntime.dylib"
                || (file_name.starts_with("libonnxruntime.") && file_name.ends_with(".dylib"))
        } else {
            file_name == "libonnxruntime.so" || file_name.starts_with("libonnxruntime.so.")
        };
        if !looks_like_runtime {
            return Err(
                "Select the ONNX Runtime shared library (onnxruntime.dll, libonnxruntime.so, or libonnxruntime.dylib)."
                    .to_owned(),
            );
        }
        let sha256 = crate::ai_masks::sha256_file_hex(&path)
            .map_err(|error| format!("Could not hash selected ONNX Runtime: {error:#}"))?;
        if let Err(error) = crate::ai_masks::probe_runtime_subprocess(&path, &sha256) {
            return Err(format!(
                "This ONNX Runtime could not be loaded safely: {error:#}"
            ));
        }
        Self::persist_onnx_runtime_selection(Some((&path, &sha256)))?;
        Ok((path, sha256))
    }

    #[cfg(not(target_os = "android"))]
    pub(crate) fn clear_onnx_runtime(&mut self) {
        match Self::persist_onnx_runtime_selection(None) {
            Ok(()) => {
                self.ai.runtime_path = None;
                self.ai.runtime_sha256 = None;
                self.ui.notice = Some(
                    "Manual ONNX Runtime selection cleared. Choose another library or switch to Automatic. Restart CalibRaw to apply the change."
                        .to_owned(),
                );
            }
            Err(error) => self.ui.notice = Some(error),
        }
    }
}
