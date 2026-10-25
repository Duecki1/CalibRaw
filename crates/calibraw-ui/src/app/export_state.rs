//! Export, batch export and library AI-refresh jobs.

use super::*;

/// A preset and a LUT applied to every photo of a batch export on top of the
/// photo's own edit. Sidecars are left unchanged.
#[derive(Clone, Debug, Default)]
pub(crate) struct ExportOverride {
    /// Already limited to what an export can apply (`Preset::for_export`).
    preset: Option<crate::presets::Preset>,
    /// Replaces the LUT of every photo.
    look_lut: Option<crate::luts::LookLut>,
}

impl ExportOverride {
    pub(crate) fn new(
        preset: Option<&crate::presets::Preset>,
        look_lut: Option<crate::luts::LookLut>,
    ) -> Self {
        Self {
            preset: preset.and_then(crate::presets::Preset::for_export),
            look_lut,
        }
    }

    /// Applies the preset, then the LUT, to `edits`. The photo keeps its own
    /// AI denoise setting, since its denoised result is not computed for an
    /// export.
    pub(crate) fn apply(&self, edits: &mut SidecarEditState) {
        if let Some(preset) = &self.preset {
            let ai_denoise_enabled = edits.exposure.ai_denoise_enabled;
            preset.apply_to(edits);
            edits.exposure.ai_denoise_enabled = ai_denoise_enabled;
        }
        if let Some(look_lut) = &self.look_lut {
            edits.look_lut = Some(look_lut.clone());
        }
    }
}

/// The preset and LUT chosen in the Library export dialog, kept for the
/// session.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ExportOverrideChoice {
    /// The preset file.
    pub(crate) preset: Option<PathBuf>,
    pub(crate) look_lut: Option<crate::luts::LutId>,
    /// 0–1.
    pub(crate) look_amount: f32,
}

impl Default for ExportOverrideChoice {
    fn default() -> Self {
        Self {
            preset: None,
            look_lut: None,
            look_amount: crate::luts::LookLut::DEFAULT_AMOUNT,
        }
    }
}

/// A photo in a folder that the batch export worker renders.
#[derive(Clone, Debug)]
pub(super) struct FileBatchExportJob {
    pub(super) source: PathBuf,
    pub(super) destination: LibraryExportDestination,
}

/// One photo of a library batch export.
#[derive(Clone, Debug)]
pub(super) enum LibraryBatchExportJob {
    /// Rendered by the batch worker straight from the file.
    File(FileBatchExportJob),
    /// Opened through the platform and exported from Develop, one at a time.
    MediaLibrary(AndroidLibraryExportTarget),
}

impl LibraryBatchExportJob {
    pub(super) fn display_name(&self) -> String {
        match self {
            Self::File(job) => job.source.display().to_string(),
            Self::MediaLibrary(target) => target.display_name().to_owned(),
        }
    }
}

/// A library photo a batch job opens in Develop.
#[derive(Clone, Debug)]
pub(super) enum LibraryJobPhoto {
    File(PathBuf),
    MediaLibrary(AndroidLibraryExportTarget),
}

impl LibraryJobPhoto {
    /// The path, or the media-library name.
    pub(super) fn label(&self) -> String {
        match self {
            Self::File(path) => path.display().to_string(),
            Self::MediaLibrary(target) => target.display_name.clone(),
        }
    }

    /// The file name, or the media-library name.
    pub(super) fn name(&self) -> String {
        match self {
            Self::File(path) => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            Self::MediaLibrary(target) => target.display_name.clone(),
        }
    }
}

/// Where a desktop library export of one photo goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LibraryExportDestination {
    /// A path from a save dialog, which already asked before replacing a file.
    Chosen(PathBuf),
    /// `{stem}.{extension}` in `folder`, numbered on when the name is taken.
    /// Nobody confirmed this name, so it never replaces an existing file.
    InFolder { folder: PathBuf, stem: String },
}

/// A media-library photo, by URI and display name.
#[derive(Clone, Debug)]
pub(crate) struct AndroidLibraryExportTarget {
    pub(crate) uri: String,
    pub(crate) display_name: String,
}

impl AndroidLibraryExportTarget {
    pub(super) fn display_name(&self) -> &str {
        &self.display_name
    }
}

#[derive(Clone, Debug)]
pub(super) struct LibraryAiMaskRefreshJob {
    pub(super) photo: LibraryJobPhoto,
    pub(super) mask_targets: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LibraryAiMaskRefreshPhase {
    Loading,
    Updating,
    Saving,
}

#[derive(Debug)]
pub(crate) struct LibraryAiMaskRefreshState {
    pub(super) pending: VecDeque<LibraryAiMaskRefreshJob>,
    pub(super) current: Option<LibraryAiMaskRefreshJob>,
    pub(super) phase: LibraryAiMaskRefreshPhase,
    pub(super) total: usize,
    pub(super) completed: usize,
    pub(super) mask_total: usize,
    pub(super) mask_completed: usize,
    pub(super) failures: Vec<String>,
    pub(super) cancel_requested: bool,
}

#[derive(Debug)]
pub(crate) struct LibraryBatchExportState {
    pub(super) pending: VecDeque<LibraryBatchExportJob>,
    pub(super) current: Option<LibraryBatchExportJob>,
    pub(super) total: usize,
    pub(super) completed: usize,
    pub(super) failures: Vec<String>,
    pub(super) cancel_requested: bool,
    /// Used by media-library batches, which start each export themselves.
    pub(super) format: ExportFormat,
    pub(super) settings: ExportSettings,
    pub(super) export_override: ExportOverride,
}

pub(super) enum LibraryBatchExportEvent {
    Started {
        job: FileBatchExportJob,
    },
    Progress {
        completed_tiles: usize,
        total_tiles: usize,
    },
    ItemFinished {
        error: Option<String>,
    },
    Finished {
        cancelled: bool,
        error: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExportTaskKind {
    Single,
    LibraryBatch,
    Replay,
}

pub(super) enum ExportTaskReceiver {
    Tiled(mpsc::Receiver<ExportEvent>),
    LibraryBatch(mpsc::Receiver<LibraryBatchExportEvent>),
    Replay(mpsc::Receiver<ReplayExportEvent>),
}

pub(super) enum ReplayExportEvent {
    Progress(crate::services::replay::ReplayProgress),
    Finished(Result<PathBuf, crate::services::replay::ReplayError>),
}

#[derive(Clone, Debug)]
pub(super) enum ExportDestination {
    File(PathBuf),
    /// A new file in `directory`; see [`crate::pipeline::ExportTarget::NewFile`].
    NewFile {
        directory: PathBuf,
        stem: String,
        extension: String,
    },
    /// A MediaStore descriptor written in place; intermediates are staged in
    /// `staging_dir`.
    AndroidDirect {
        path: PathBuf,
        staging_dir: PathBuf,
    },
    AndroidGallery {
        path: PathBuf,
        display_name: String,
        format: ExportFormat,
    },
}

impl ExportDestination {
    pub(super) fn target(&self) -> crate::pipeline::ExportTarget {
        use crate::pipeline::ExportTarget;
        match self {
            Self::File(path) => ExportTarget::File(path.clone()),
            Self::NewFile {
                directory,
                stem,
                extension,
            } => ExportTarget::NewFile {
                directory: directory.clone(),
                stem: stem.clone(),
                extension: extension.clone(),
            },
            Self::AndroidDirect { path, staging_dir } => ExportTarget::Descriptor {
                path: path.clone(),
                staging_dir: staging_dir.clone(),
            },
            // Gallery exports are cached as files and published afterwards.
            Self::AndroidGallery { path, .. } => ExportTarget::File(path.clone()),
        }
    }
}

pub(crate) struct ExportTask {
    pub(super) kind: ExportTaskKind,
    pub(super) cancellation: Arc<std::sync::atomic::AtomicBool>,
    pub(super) receiver: Option<ExportTaskReceiver>,
    pub(super) destination: Option<ExportDestination>,
    pub(super) progress: f32,
    pub(super) phase: String,
    pub(super) completed: usize,
    pub(super) total: usize,
    pub(super) completed_tiles: usize,
    pub(super) total_tiles: usize,
    pub(super) cancelling: bool,
}

pub(super) struct PreparedExportSource {
    pub(super) raw: Arc<LoadedRaw>,
    pub(super) geometry: GeometryTransform,
    pub(super) exposure: ExposureParams,
    pub(super) masks: MaskStack,
    pub(super) remove: RemoveEditState,
    pub(super) look: Option<crate::luts::ResolvedLook>,
    pub(super) source_file_name: Option<String>,
    pub(super) gpu_export_prewarm: Option<Arc<GpuProgramPrewarm>>,
}

pub(super) struct ExportItemRequest {
    pub(super) device: wgpu::Device,
    pub(super) queue: wgpu::Queue,
    pub(super) source: PreparedExportSource,
    pub(super) destination: ExportDestination,
    pub(super) format: ExportFormat,
    pub(super) settings: ExportSettings,
}

pub(super) struct LensCorrectionTaskRequest {
    pub(super) original_raw: Arc<LoadedRaw>,
    pub(super) selection: Option<LensCorrection>,
    pub(super) preview_quality: PreviewQuality,
    pub(super) preview_proxy_edge: u32,
    pub(super) cached_raws: Option<(Arc<LoadedRaw>, Arc<LoadedRaw>)>,
}

pub(crate) struct ExportState {
    pub(crate) format: ExportFormat,
    pub(crate) gpu_prewarm: Option<Arc<GpuProgramPrewarm>>,
    pub(crate) settings: ExportSettings,
    pub(crate) task: Option<ExportTask>,
    pub(crate) batch: Option<LibraryBatchExportState>,
    pub(crate) publish_pending: bool,
    /// The platform is opening a photo for the media-library batch export.
    pub(crate) android_batch_load_pending: bool,
    /// MIME type to share the next finished single export or replay as; `None`
    /// publishes without sharing. Kept outside the task because a replay's task
    /// ends before its gallery publish does.
    pub(crate) share_mime_type: Option<&'static str>,
    pub(crate) override_choice: ExportOverrideChoice,
}
