//! Export, batch export and library AI-refresh jobs.

use super::*;

#[cfg(not(target_os = "android"))]
#[derive(Clone, Debug)]
pub(super) struct LibraryBatchExportJob {
    pub(super) source: PathBuf,
    pub(super) destination: LibraryExportDestination,
}

/// Where a desktop library export of one photo goes.
#[cfg(not(target_os = "android"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LibraryExportDestination {
    /// A path from a save dialog, which already asked before replacing a file.
    Chosen(PathBuf),
    /// `{stem}.{extension}` in `folder`, numbered on when the name is taken.
    /// Nobody confirmed this name, so it never replaces an existing file.
    InFolder { folder: PathBuf, stem: String },
}

#[cfg(target_os = "android")]
#[derive(Clone, Debug)]
pub(crate) struct AndroidLibraryExportTarget {
    pub(crate) uri: String,
    pub(crate) display_name: String,
}

#[cfg(target_os = "android")]
impl AndroidLibraryExportTarget {
    pub(super) fn display_name(&self) -> &str {
        &self.display_name
    }
}

#[cfg(target_os = "android")]
#[derive(Clone, Debug)]
pub(super) struct LibraryBatchExportJob {
    pub(super) target: AndroidLibraryExportTarget,
}

#[cfg(not(target_os = "android"))]
#[derive(Clone, Debug)]
pub(super) struct LibraryAiMaskRefreshJob {
    pub(super) source: PathBuf,
    pub(super) mask_targets: usize,
}

#[cfg(target_os = "android")]
#[derive(Clone, Debug)]
pub(super) struct LibraryAiMaskRefreshJob {
    pub(super) uri: String,
    pub(super) display_name: String,
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
    #[cfg(target_os = "android")]
    pub(super) format: ExportFormat,
    #[cfg(target_os = "android")]
    pub(super) settings: ExportSettings,
}

#[cfg(not(target_os = "android"))]
pub(super) enum LibraryBatchExportEvent {
    Started {
        job: LibraryBatchExportJob,
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
    #[cfg(not(target_os = "android"))]
    LibraryBatch(mpsc::Receiver<LibraryBatchExportEvent>),
    Replay(mpsc::Receiver<ReplayExportEvent>),
}

pub(super) enum ReplayExportEvent {
    Progress(crate::services::replay::ReplayProgress),
    Finished(Result<PathBuf, crate::services::replay::ReplayError>),
}

#[derive(Clone, Debug)]
pub(super) enum ExportDestination {
    #[cfg(not(target_os = "android"))]
    File(PathBuf),
    /// A new file in `directory`; see [`crate::pipeline::ExportTarget::NewFile`].
    #[cfg(not(target_os = "android"))]
    NewFile {
        directory: PathBuf,
        stem: String,
        extension: String,
    },
    /// A MediaStore descriptor written in place; intermediates are staged in
    /// `staging_dir`.
    #[cfg(target_os = "android")]
    AndroidDirect { path: PathBuf, staging_dir: PathBuf },
    #[cfg(target_os = "android")]
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
            #[cfg(not(target_os = "android"))]
            Self::File(path) => ExportTarget::File(path.clone()),
            #[cfg(not(target_os = "android"))]
            Self::NewFile {
                directory,
                stem,
                extension,
            } => ExportTarget::NewFile {
                directory: directory.clone(),
                stem: stem.clone(),
                extension: extension.clone(),
            },
            #[cfg(target_os = "android")]
            Self::AndroidDirect { path, staging_dir } => ExportTarget::Descriptor {
                path: path.clone(),
                staging_dir: staging_dir.clone(),
            },
            // Gallery exports are cached as files and published afterwards.
            #[cfg(target_os = "android")]
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
    pub(super) minimized: bool,
    pub(super) cancelling: bool,
    /// MIME type to share the finished export as; `None` exports without sharing.
    #[cfg(target_os = "android")]
    pub(super) share_mime_type: Option<&'static str>,
}

pub(super) struct PreparedExportSource {
    pub(super) raw: Arc<LoadedRaw>,
    pub(super) geometry: GeometryTransform,
    pub(super) exposure: ExposureParams,
    pub(super) masks: MaskStack,
    pub(super) remove: RemoveEditState,
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
    pub(super) selection: Option<LensfunLens>,
    #[cfg(target_os = "android")]
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
    #[cfg(target_os = "android")]
    pub(crate) android_batch_load_pending: bool,
}
