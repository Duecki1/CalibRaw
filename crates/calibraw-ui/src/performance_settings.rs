use crate::pipeline::{CameraProfileMode, ExportFormat};
use crate::platform::PlatformCaps;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

// Bump when a public settings change needs migration; see `sanitized`.
// Version 5 dropped the automatic runtime choice (`onnx_runtime_kind` and
// `onnx_runtime_kind_chosen`), which unknown fields leave to be ignored.
// Version 6 moved the Preview quality tiers of memory-constrained devices;
// see `sanitized`.
const SETTINGS_VERSION: u32 = 6;
/// The desktop decoded-RAW cache default before version 2 raised it, so that
/// Develop can prefetch both neighbours of the open photo.
///
/// migration: remove in v2.0.0, with the version 2 step in `sanitized`.
const DESKTOP_RAW_CACHE_FILES_BEFORE_V2: usize = 2;
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
    /// Render the idle preview from the whole native photo; where the platform offers it.
    pub preview_full_resolution: bool,
    pub image_relative_brush_size: bool,
    pub show_develop_navigation_labels: bool,
    /// The Develop cards of the sidebar layout, in the order shown.
    pub develop_card_order: crate::app::DevelopCardOrder,
    pub export_name_template: String,
    /// Last folder chosen for a desktop image export.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_export_directory: Option<PathBuf>,
    pub export_conflict_behavior: ExportConflictBehavior,
    #[serde(with = "export_format_serde")]
    pub export_format: ExportFormat,
    pub ui_design: crate::appearance::UiDesign,
    pub preview_backdrop: crate::appearance::PreviewBackdrop,
    pub onboarding_completed: bool,
    /// Missing in files written before the setup step existed, so existing
    /// users are asked once.
    pub subject_mask_quality_chosen: bool,
    /// Show a feature's tip the first time it is used.
    pub tips_enabled: bool,
    /// `Tip::key`s of the tips already shown, including keys of tips other
    /// versions know.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub seen_tips: BTreeSet<String>,
    pub auto_check_updates: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github_update_check_allowed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignored_update_version: Option<String>,
    pub subject_mask_quality: calibraw_ai::models::subject::SubjectMaskQuality,
    // The fields below are read only where the platform offers what they
    // configure; every platform writes all of them.
    /// Where the AI runtime is configurable.
    pub ai_gpu_acceleration: bool,
    /// Where the AI runtime is configurable.
    pub onnx_runtime_mode: crate::app::OnnxRuntimeMode,
    /// Where Discord can be reached.
    pub discord_rich_presence: bool,
    /// Serialize photo file reads for libraries on rotational disks; folder
    /// storage only.
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
    /// Ask about manual correction when automatic correction finds no profile.
    pub missing_lens_profile_prompt: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_photo_preset: Option<PathBuf>,
    pub adjustment_copy_settings: crate::sidecar::AdjustmentCopySettings,
    /// The media-library folder open last; media-library storage only.
    pub(crate) last_android_library_folder: String,
    /// The folder library's root and selected folder; folder storage only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_library_folder: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_library_selected_folder: Option<PathBuf>,
    /// Desktop window layout.
    pub library_folder_sidebar_open: bool,
    pub develop_filmstrip_open: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExportConflictBehavior {
    #[default]
    AddNumber,
    Overwrite,
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
            preview_full_resolution: false,
            image_relative_brush_size: false,
            show_develop_navigation_labels: true,
            develop_card_order: crate::app::DevelopCardOrder::default(),
            export_name_template: crate::export_naming::DEFAULT_EXPORT_NAME_TEMPLATE.to_owned(),
            last_export_directory: None,
            export_conflict_behavior: ExportConflictBehavior::default(),
            export_format: ExportFormat::Jpeg,
            ui_design: crate::appearance::UiDesign::default(),
            preview_backdrop: crate::appearance::PreviewBackdrop::default(),
            onboarding_completed: false,
            subject_mask_quality_chosen: false,
            tips_enabled: true,
            seen_tips: BTreeSet::new(),
            auto_check_updates: true,
            github_update_check_allowed: None,
            ignored_update_version: None,
            subject_mask_quality: calibraw_ai::models::subject::SubjectMaskQuality::default(),
            ai_gpu_acceleration: true,
            onnx_runtime_mode: crate::app::OnnxRuntimeMode::default(),
            discord_rich_presence: false,
            hdd_mode: false,
            camera_profile_mode: CameraProfileMode::default(),
            camera_profile_folder: None,
            camera_profile_folder_label: None,
            // Folder storage finds profiles in the standard folders;
            // a media library imports them instead.
            camera_profile_auto_detect: PlatformCaps::CURRENT.has_folders(),
            last_camera_profile: None,
            automatic_lens_correction: true,
            automatic_lens_geometry: true,
            automatic_lens_vignetting: false,
            missing_lens_profile_prompt: true,
            new_photo_preset: None,
            adjustment_copy_settings: crate::sidecar::AdjustmentCopySettings::default(),
            last_android_library_folder: String::new(),
            last_library_folder: None,
            last_library_selected_folder: None,
            library_folder_sidebar_open: true,
            develop_filmstrip_open: true,
        }
    }
}

/// The tier that keeps the density of a Preview quality stored before settings
/// version 6 on a memory-constrained device; elsewhere the tiers kept their
/// densities and the stored choice is kept as it is.
// migration: remove in v2.0.0. Listed in `calibraw_core`'s `migrations`.
fn preview_quality_before_v6(
    stored: crate::app::PreviewQuality,
    constrained_memory: bool,
) -> crate::app::PreviewQuality {
    use crate::app::PreviewQuality::{High, Low, Max, Medium};
    if !constrained_memory {
        return stored;
    }
    match stored {
        Low => Medium,
        Medium | High | Max => High,
    }
}

impl PerformanceSettings {
    pub(crate) fn sanitized(mut self) -> Self {
        // Version 1 is the public baseline. Apply public-version migrations here before updating
        // the stored version, then keep the value sanitization below version-agnostic.
        //
        // Version 2: every save wrote the cache size, so a stored old default is almost always
        // the default rather than a choice; move it to the new one.
        // migration: remove in v2.0.0. Listed in `calibraw_core`'s `migrations`.
        if self.version < 2
            && !PlatformCaps::CURRENT.constrained_memory()
            && self.raw_cache_files == DESKTOP_RAW_CACHE_FILES_BEFORE_V2
        {
            self.raw_cache_files = crate::app::default_raw_cache_limit();
        }
        // Version 3: automatic vignetting correction is off by default. Every save wrote the old
        // default, so it is turned off unless it was the only correction chosen.
        // migration: remove in v2.0.0. Listed in `calibraw_core`'s `migrations`.
        if self.version < 3 && self.automatic_lens_geometry {
            self.automatic_lens_vignetting = false;
        }
        // Version 4: Develop navigation labels are on by default. Every save wrote the old
        // default, so they are turned on once.
        // migration: remove in v2.0.0. Listed in `calibraw_core`'s `migrations`.
        if self.version < 4 {
            self.show_develop_navigation_labels = true;
        }
        // Version 6: a memory-constrained device's Preview quality tiers became
        // densities relative to the screen, topping out at one rendered pixel per
        // screen pixel. It never rendered the old tiers — its preview was capped
        // near 1050 pixels whatever they said — so they become the tier a new
        // install starts with, except old Low, which keeps its nominal 75% as the
        // new Medium. A computer kept the old densities (Low 75%, Medium 100%,
        // High 125%, Max 150%), so there the stored tier keeps its meaning.
        // migration: remove in v2.0.0. Listed in `calibraw_core`'s `migrations`.
        if self.version < 6 {
            self.preview_quality = preview_quality_before_v6(
                self.preview_quality,
                PlatformCaps::CURRENT.constrained_memory(),
            );
        }
        self.version = SETTINGS_VERSION;
        // Setup offers the default until the user has chosen; earlier builds
        // saved other defaults.
        if !self.subject_mask_quality_chosen
            || !calibraw_ai::models::subject::SubjectMaskQuality::AVAILABLE
                .contains(&self.subject_mask_quality)
        {
            self.subject_mask_quality = calibraw_ai::models::subject::SubjectMaskQuality::default();
        }
        self.raw_cache_files = self
            .raw_cache_files
            .min(crate::app::maximum_raw_cache_limit());
        self.thumbnail_workers = self
            .thumbnail_workers
            .clamp(1, crate::ui::library::maximum_thumbnail_worker_count());
        if !self.automatic_lens_geometry && !self.automatic_lens_vignetting {
            let defaults = Self::default();
            self.automatic_lens_geometry = defaults.automatic_lens_geometry;
            self.automatic_lens_vignetting = defaults.automatic_lens_vignetting;
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
