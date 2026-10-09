use crate::pipeline::{CameraProfileMode, ExportFormat};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// First public settings layout. Bump when a public settings change needs migration.
const SETTINGS_VERSION: u32 = 1;
const MAX_SETTINGS_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct PerformanceSettings {
    pub develop_histogram_open: bool,
    pub app_usage_ms: u64,
    pub version: u32,
    pub raw_cache_files: usize,
    /// The manual thumbnail worker limit, used while `thumbnail_workers_automatic` is off.
    pub thumbnail_workers: usize,
    /// Size the thumbnail worker pool from the machine's cores and memory.
    /// Missing in files written before it existed, which therefore turn it on.
    pub thumbnail_workers_automatic: bool,
    pub render_edited_thumbnails_during_indexing: bool,
    pub library_thumbnail_size: crate::ui::library::LibraryThumbnailSize,
    pub library_sort_order: crate::ui::library::LibrarySortOrder,
    /// Show RAW+JPEG (or RAW+HEIC) pairs once, as the RAW.
    pub library_stack_raw_companions: bool,
    pub preview_quality: crate::app::PreviewQuality,
    pub image_relative_brush_size: bool,
    pub show_develop_navigation_labels: bool,
    pub export_name_template: String,
    #[serde(with = "export_format_serde")]
    pub export_format: ExportFormat,
    pub ui_design: crate::appearance::UiDesign,
    pub preview_backdrop: crate::appearance::PreviewBackdrop,
    pub onboarding_completed: bool,
    pub auto_check_updates: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github_update_check_allowed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignored_update_version: Option<String>,
    pub birefnet_quality: calibraw_ai::ai_masks::BiRefNetQuality,
    #[cfg(not(target_os = "android"))]
    pub subject_crop_refinement: bool,
    #[cfg(not(target_os = "android"))]
    pub ai_gpu_acceleration: bool,
    #[cfg(not(target_os = "android"))]
    pub onnx_runtime_mode: crate::app::OnnxRuntimeMode,
    #[cfg(not(target_os = "android"))]
    pub discord_rich_presence: bool,
    /// Serialize photo file reads for libraries on rotational disks.
    #[cfg(not(target_os = "android"))]
    pub hdd_mode: bool,
    pub camera_profile_mode: CameraProfileMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera_profile_folder: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera_profile_folder_label: Option<String>,
    pub camera_profile_auto_detect: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_camera_profile: Option<PathBuf>,
    /// Apply the matched Lensfun profile when a photo without saved edits opens.
    pub automatic_lens_correction: bool,
    pub automatic_lens_geometry: bool,
    pub automatic_lens_vignetting: bool,
    pub adjustment_copy_settings: crate::sidecar::AdjustmentCopySettings,
    #[cfg(target_os = "android")]
    pub(crate) last_android_library_folder: String,
    #[cfg(not(target_os = "android"))]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_library_folder: Option<PathBuf>,
    #[cfg(not(target_os = "android"))]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_library_selected_folder: Option<PathBuf>,
    #[cfg(not(target_os = "android"))]
    pub library_folder_sidebar_open: bool,
    #[cfg(not(target_os = "android"))]
    pub develop_filmstrip_open: bool,
}

mod export_format_serde {
    use super::ExportFormat;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S>(format: &ExportFormat, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(match format {
            ExportFormat::Jpeg => "jpeg",
            ExportFormat::Png => "png",
            ExportFormat::Tiff => "tiff",
            ExportFormat::JpegXl => "jxl",
        })
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<ExportFormat, D::Error>
    where
        D: Deserializer<'de>,
    {
        match String::deserialize(deserializer)?.as_str() {
            "jpeg" => Ok(ExportFormat::Jpeg),
            "png" => Ok(ExportFormat::Png),
            "tiff" => Ok(ExportFormat::Tiff),
            "jxl" => Ok(ExportFormat::JpegXl),
            value => Err(serde::de::Error::unknown_variant(
                value,
                &["jpeg", "png", "tiff", "jxl"],
            )),
        }
    }
}

#[cfg(test)]
#[test]
fn jpeg_xl_export_format_setting_round_trips() {
    let encoded = serde_json::to_string(&ExportFormatSetting(ExportFormat::JpegXl)).unwrap();
    assert_eq!(encoded, "\"jxl\"");
    let decoded: ExportFormatSetting = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded.0, ExportFormat::JpegXl);
}

#[cfg(test)]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
struct ExportFormatSetting(#[serde(with = "export_format_serde")] ExportFormat);

const fn subject_quality_for_platform(
    configured: calibraw_ai::ai_masks::BiRefNetQuality,
    android: bool,
) -> calibraw_ai::ai_masks::BiRefNetQuality {
    if android {
        calibraw_ai::ai_masks::BiRefNetQuality::Low
    } else {
        configured
    }
}

impl Default for PerformanceSettings {
    fn default() -> Self {
        Self {
            develop_histogram_open: false,
            app_usage_ms: 0,
            version: SETTINGS_VERSION,
            raw_cache_files: crate::app::default_raw_cache_limit(),
            thumbnail_workers: crate::ui::library::default_thumbnail_worker_count(),
            thumbnail_workers_automatic: true,
            render_edited_thumbnails_during_indexing: false,
            library_thumbnail_size: crate::ui::library::LibraryThumbnailSize::default(),
            library_sort_order: crate::ui::library::LibrarySortOrder::default(),
            library_stack_raw_companions: true,
            preview_quality: crate::app::PreviewQuality::default(),
            image_relative_brush_size: false,
            show_develop_navigation_labels: false,
            export_name_template: crate::export_naming::DEFAULT_EXPORT_NAME_TEMPLATE.to_owned(),
            export_format: ExportFormat::Jpeg,
            ui_design: crate::appearance::UiDesign::default(),
            preview_backdrop: crate::appearance::PreviewBackdrop::default(),
            onboarding_completed: false,
            auto_check_updates: true,
            github_update_check_allowed: None,
            ignored_update_version: None,
            birefnet_quality: calibraw_ai::ai_masks::BiRefNetQuality::default(),
            #[cfg(not(target_os = "android"))]
            subject_crop_refinement: true,
            #[cfg(not(target_os = "android"))]
            ai_gpu_acceleration: true,
            #[cfg(not(target_os = "android"))]
            onnx_runtime_mode: crate::app::OnnxRuntimeMode::default(),
            #[cfg(not(target_os = "android"))]
            discord_rich_presence: false,
            #[cfg(not(target_os = "android"))]
            hdd_mode: false,
            camera_profile_mode: CameraProfileMode::default(),
            camera_profile_folder: None,
            camera_profile_folder_label: None,
            camera_profile_auto_detect: !cfg!(target_os = "android"),
            last_camera_profile: None,
            automatic_lens_correction: true,
            automatic_lens_geometry: true,
            automatic_lens_vignetting: true,
            adjustment_copy_settings: crate::sidecar::AdjustmentCopySettings::default(),
            #[cfg(target_os = "android")]
            last_android_library_folder: String::new(),
            #[cfg(not(target_os = "android"))]
            last_library_folder: None,
            #[cfg(not(target_os = "android"))]
            last_library_selected_folder: None,
            #[cfg(not(target_os = "android"))]
            library_folder_sidebar_open: true,
            #[cfg(not(target_os = "android"))]
            develop_filmstrip_open: true,
        }
    }
}

impl PerformanceSettings {
    pub(crate) fn sanitized(mut self) -> Self {
        // Version 1 is the public baseline. Apply future public-version migrations here before
        // updating the stored version, then keep the value sanitization below version-agnostic.
        self.version = SETTINGS_VERSION;
        self.raw_cache_files = self
            .raw_cache_files
            .min(crate::app::maximum_raw_cache_limit());
        self.thumbnail_workers = self
            .thumbnail_workers
            .clamp(1, crate::ui::library::maximum_thumbnail_worker_count());
        self.birefnet_quality =
            subject_quality_for_platform(self.birefnet_quality, cfg!(target_os = "android"));
        if !self.automatic_lens_geometry && !self.automatic_lens_vignetting {
            self.automatic_lens_geometry = true;
            self.automatic_lens_vignetting = true;
        }
        if self.github_update_check_allowed == Some(false) {
            self.auto_check_updates = false;
        }
        self.export_name_template =
            crate::export_naming::sanitize_template_setting(&self.export_name_template);
        self
    }
}

pub(crate) fn load(path: Option<&Path>) -> PerformanceSettings {
    let Some(path) = path else {
        return PerformanceSettings::default();
    };
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_SETTINGS_BYTES => {}
        Ok(_) => return PerformanceSettings::default(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return PerformanceSettings::default()
        }
        Err(error) => {
            log::warn!(
                "could not inspect performance settings {}: {error}",
                path.display()
            );
            return PerformanceSettings::default();
        }
    }
    match std::fs::read(path)
        .map_err(|error| error.to_string())
        .and_then(|bytes| {
            serde_json::from_slice::<PerformanceSettings>(&bytes).map_err(|error| error.to_string())
        }) {
        Ok(settings) => settings.sanitized(),
        Err(error) => {
            log::warn!(
                "could not load performance settings {}: {error}",
                path.display()
            );
            PerformanceSettings::default()
        }
    }
}

pub(crate) fn save(path: Option<&Path>, settings: PerformanceSettings) -> Result<(), String> {
    let Some(path) = path else {
        return Ok(());
    };
    let settings = settings.sanitized();
    let bytes = serde_json::to_vec_pretty(&settings)
        .map_err(|error| format!("could not encode performance settings: {error}"))?;
    calibraw_core::file_ops::write_bytes_atomically(path, &bytes).map_err(|error| {
        format!(
            "could not save performance settings {}: {error}",
            path.display()
        )
    })
}

#[cfg(not(target_os = "android"))]
pub(crate) fn desktop_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);

    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Library/Application Support"));

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "android")))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".config"))
        });

    #[cfg(not(any(windows, unix, target_os = "macos")))]
    let base: Option<PathBuf> = None;

    base.map(|base| base.join("calibraw").join("performance.json"))
}

#[cfg(not(target_os = "android"))]
pub(crate) fn detected_camera_profile_folder() -> Option<PathBuf> {
    camera_profile_candidates()
        .into_iter()
        .find(|path| path.is_dir())
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn camera_profile_candidates_under(root: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![root.join("CameraRaw").join("CameraProfiles")];
    if let Ok(entries) = std::fs::read_dir(root) {
        candidates.extend(
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path().join("CameraRaw").join("CameraProfiles")),
        );
    }
    candidates.sort();
    candidates.dedup();
    candidates
}

#[cfg(not(target_os = "android"))]
pub(crate) fn camera_profile_candidates() -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        return if let Some(program_data) =
            std::env::var_os("ProgramData").or_else(|| std::env::var_os("ALLUSERSPROFILE"))
        {
            camera_profile_candidates_under(&PathBuf::from(program_data))
        } else {
            Vec::new()
        };
    }

    #[cfg(target_os = "macos")]
    {
        let mut candidates = Vec::new();
        if let Some(home) = std::env::var_os("HOME") {
            candidates.extend(camera_profile_candidates_under(
                &PathBuf::from(home)
                    .join("Library")
                    .join("Application Support"),
            ));
        }
        candidates.extend(camera_profile_candidates_under(&PathBuf::from(
            "/Library/Application Support",
        )));
        return candidates;
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "android")))]
    {
        Vec::new()
    }

    #[cfg(not(any(windows, unix, target_os = "macos")))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_values_are_clamped() {
        for (thumbnail_workers, expected_workers) in [
            (0, 1),
            (
                usize::MAX,
                crate::ui::library::maximum_thumbnail_worker_count(),
            ),
        ] {
            let settings = PerformanceSettings {
                version: 99,
                raw_cache_files: usize::MAX,
                thumbnail_workers,
                ..Default::default()
            }
            .sanitized();

            assert_eq!(settings.version, SETTINGS_VERSION);
            assert_eq!(
                settings.raw_cache_files,
                crate::app::maximum_raw_cache_limit()
            );
            assert_eq!(settings.thumbnail_workers, expected_workers);
        }
    }

    #[test]
    fn denied_github_permission_disables_automatic_checks() {
        let settings = PerformanceSettings {
            auto_check_updates: true,
            github_update_check_allowed: Some(false),
            ..Default::default()
        }
        .sanitized();

        assert!(!settings.auto_check_updates);
        assert_eq!(settings.github_update_check_allowed, Some(false));
    }

    #[test]
    fn github_permission_choice_survives_disk_round_trip() {
        let path = std::env::temp_dir().join(format!(
            "calibraw-version-consent-{}-{:?}.json",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);

        let allowed = PerformanceSettings {
            auto_check_updates: true,
            github_update_check_allowed: Some(true),
            ..Default::default()
        };
        save(Some(&path), allowed).expect("allowed GitHub preference should save");
        let restored = load(Some(&path));
        assert_eq!(restored.github_update_check_allowed, Some(true));
        assert!(restored.auto_check_updates);

        let denied = PerformanceSettings {
            auto_check_updates: true,
            github_update_check_allowed: Some(false),
            ..restored
        };
        save(Some(&path), denied).expect("denied GitHub preference should save");
        let restored = load(Some(&path));
        assert_eq!(restored.github_update_check_allowed, Some(false));
        assert!(!restored.auto_check_updates);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn omitted_settings_fields_use_current_defaults() {
        let empty: PerformanceSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(
            serde_json::to_value(&empty).unwrap(),
            serde_json::to_value(PerformanceSettings::default()).unwrap()
        );

        assert!(
            empty.automatic_lens_correction
                && empty.automatic_lens_geometry
                && empty.automatic_lens_vignetting
        );

        let settings: PerformanceSettings =
            serde_json::from_str(r#"{"version":1,"raw_cache_files":1,"thumbnail_workers":1}"#)
                .expect("baseline settings should remain readable");

        assert_eq!(SETTINGS_VERSION, 1);
        // Files written before automatic workers existed switch to automatic and
        // keep their stored count as the manual limit.
        assert!(settings.thumbnail_workers_automatic);
        assert_eq!(settings.thumbnail_workers, 1);
        assert_eq!(settings.preview_quality, crate::app::PreviewQuality::Medium);
        assert!(!settings.image_relative_brush_size);
        assert!(!settings.show_develop_navigation_labels);
        assert!(!settings.develop_histogram_open);
        assert_eq!(settings.app_usage_ms, 0);
        assert_eq!(
            settings.export_name_template,
            crate::export_naming::DEFAULT_EXPORT_NAME_TEMPLATE
        );
        assert_eq!(settings.export_format, ExportFormat::Jpeg);
        assert_eq!(
            settings.ui_design,
            crate::appearance::UiDesign::PlainGreyDark
        );
        assert_eq!(
            settings.preview_backdrop,
            crate::appearance::PreviewBackdrop::DarkGrey
        );
        assert!(!settings.render_edited_thumbnails_during_indexing);
        assert!(settings.auto_check_updates);
        assert!(settings.github_update_check_allowed.is_none());
        assert!(settings.ignored_update_version.is_none());
        assert_eq!(
            settings.birefnet_quality,
            calibraw_ai::ai_masks::BiRefNetQuality::Low
        );
        assert_eq!(
            settings.library_thumbnail_size,
            crate::ui::library::LibraryThumbnailSize::Large
        );
        assert_eq!(
            settings.library_sort_order,
            crate::ui::library::LibrarySortOrder::NewestFirst
        );
        assert!(settings.library_stack_raw_companions);
        #[cfg(not(target_os = "android"))]
        {
            assert!(settings.subject_crop_refinement);
            assert!(settings.ai_gpu_acceleration);
            assert_eq!(
                settings.onnx_runtime_mode,
                crate::app::OnnxRuntimeMode::Automatic
            );
            assert!(!settings.discord_rich_presence);
            assert!(!settings.hdd_mode);
            assert!(settings.library_folder_sidebar_open);
            assert!(settings.develop_filmstrip_open);
        }
        assert_eq!(
            settings.adjustment_copy_settings,
            crate::sidecar::AdjustmentCopySettings::default()
        );
        assert!(!settings.onboarding_completed);
    }

    #[test]
    fn library_preferences_round_trip() {
        let mut settings = PerformanceSettings {
            develop_histogram_open: true,
            library_thumbnail_size: crate::ui::library::LibraryThumbnailSize::Enormous,
            library_sort_order: crate::ui::library::LibrarySortOrder::SmallestFirst,
            library_stack_raw_companions: false,
            birefnet_quality: calibraw_ai::ai_masks::BiRefNetQuality::High,
            image_relative_brush_size: true,
            show_develop_navigation_labels: true,
            export_name_template: "{OriginalName}-{CurrentDate}".to_owned(),
            export_format: ExportFormat::Png,
            ui_design: crate::appearance::UiDesign::Porcelain,
            preview_backdrop: crate::appearance::PreviewBackdrop::MatchPhoto,
            render_edited_thumbnails_during_indexing: true,
            thumbnail_workers_automatic: false,
            automatic_lens_correction: false,
            automatic_lens_vignetting: false,
            ..Default::default()
        };
        #[cfg(not(target_os = "android"))]
        {
            settings.subject_crop_refinement = false;
            settings.ai_gpu_acceleration = false;
            settings.onnx_runtime_mode = crate::app::OnnxRuntimeMode::Manual;
            settings.discord_rich_presence = true;
            settings.hdd_mode = true;
            settings.last_library_folder = Some(PathBuf::from("photos"));
            settings.last_library_selected_folder = Some(PathBuf::from("photos/2026/trip"));
            settings.library_folder_sidebar_open = false;
            settings.develop_filmstrip_open = false;
        }

        let json = serde_json::to_string(&settings).unwrap();
        let restored: PerformanceSettings = serde_json::from_str(&json).unwrap();

        assert_eq!(
            restored.library_thumbnail_size,
            crate::ui::library::LibraryThumbnailSize::Enormous
        );
        assert_eq!(
            restored.library_sort_order,
            crate::ui::library::LibrarySortOrder::SmallestFirst
        );
        assert!(!restored.library_stack_raw_companions);
        assert_eq!(
            restored.birefnet_quality,
            calibraw_ai::ai_masks::BiRefNetQuality::High
        );
        assert!(restored.image_relative_brush_size);
        assert!(restored.show_develop_navigation_labels);
        assert!(!restored.automatic_lens_correction);
        assert!(restored.automatic_lens_geometry && !restored.automatic_lens_vignetting);
        assert!(restored.develop_histogram_open);
        assert_eq!(
            restored.export_name_template,
            "{OriginalName}-{CurrentDate}"
        );
        assert_eq!(restored.export_format, ExportFormat::Png);
        assert_eq!(restored.ui_design, crate::appearance::UiDesign::Porcelain);
        assert_eq!(
            restored.preview_backdrop,
            crate::appearance::PreviewBackdrop::MatchPhoto
        );
        assert!(restored.render_edited_thumbnails_during_indexing);
        assert!(!restored.thumbnail_workers_automatic);
        #[cfg(not(target_os = "android"))]
        {
            assert!(!restored.subject_crop_refinement);
            assert!(!restored.ai_gpu_acceleration);
            assert_eq!(
                restored.onnx_runtime_mode,
                crate::app::OnnxRuntimeMode::Manual
            );
            assert!(restored.discord_rich_presence);
            assert!(restored.hdd_mode);
            assert_eq!(restored.last_library_folder, Some(PathBuf::from("photos")));
            assert_eq!(
                restored.last_library_selected_folder,
                Some(PathBuf::from("photos/2026/trip"))
            );
            assert!(!restored.library_folder_sidebar_open);
            assert!(!restored.develop_filmstrip_open);
        }
    }

    #[test]
    fn android_always_sanitizes_subject_quality_to_low() {
        assert_eq!(
            subject_quality_for_platform(calibraw_ai::ai_masks::BiRefNetQuality::High, true),
            calibraw_ai::ai_masks::BiRefNetQuality::Low
        );
        assert_eq!(
            subject_quality_for_platform(calibraw_ai::ai_masks::BiRefNetQuality::High, false),
            calibraw_ai::ai_masks::BiRefNetQuality::High
        );
    }
}
