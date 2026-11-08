use crate::app::CalibRawApp;
use crate::app::LibraryExportDestination;
use crate::i18n::{tr, tr_fmt};
use crate::pipeline::{
    is_supported_image_path, load_raw_display_metadata, load_raw_thumbnail,
    load_raw_thumbnail_with_display_metadata,
};
use crate::pipeline::{ExportFormat, ExportSettings, RawThumbnail};
use crate::platform::{Platform, PlatformCaps};
use eframe::egui::{self, Align2, Color32, FontId, Sense, Stroke, StrokeKind, Ui};
use std::cmp::Ordering as CmpOrdering;
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

mod actions;
mod adjustments;
mod catalog;
mod clipboard;
mod companions;
mod deferred_metadata;
mod dialogs;
mod export;
mod filter;
mod folders;
mod hover;
mod keys;
mod local;
mod media_library;
mod merge;
mod review;
mod sources;
mod state;
pub(crate) use state::LibraryPreferences;
mod storage;
mod thumbnails;
mod view;
use hover::thumbnail_hover_overlay;
pub(crate) use review::{
    current_document_id, current_photo_asset, paint_review_badge, show_current_photo_review,
    show_review_feedback, take_review_key, ReviewChange, ReviewFeedback,
};
pub(crate) use state::ReviewSave;

use actions::*;
use adjustments::*;
use catalog::*;
use clipboard::*;
use companions::*;
use deferred_metadata::*;
use dialogs::*;
use export::*;
use filter::*;
use folders::catalog::*;
use folders::files::*;
use keys::*;
use media_library::catalog::*;
use media_library::{
    android_folder_ancestors, android_folder_parent, android_library_location_label,
};
use sources::*;
use storage::*;
use thumbnails::*;

pub(crate) use actions::photo_actions_menu;
pub(crate) use actions::{apply_library_action, show_library_action_overlays};
pub(crate) use companions::{
    corner_inset_beside, paint_edit_badge, paint_format_badge, rendered_format_label, EditBadge,
};
pub(crate) use merge::{MergeKind, MergeOutcome};
pub(crate) use thumbnails::{load_desktop_cached_thumbnail, load_desktop_reference_preview};
pub(crate) use view::Library;

const THUMBNAIL_EDGE: u32 = 512;
const MAX_LIBRARY_FILES: usize = 20_000;
const MAX_EVENTS_PER_FRAME: usize = 12;
const MAX_PENDING_THUMBNAIL_RESULTS: usize = 32;
const MAX_PENDING_THUMBNAILS: usize = 64;
const DESKTOP_TEXTURE_CACHE_LIMIT: usize = 128;
const ANDROID_TEXTURE_CACHE_LIMIT: usize = 48;
const RESIDENT_THUMBNAIL_EDGE: u32 = 384;
const DESKTOP_RESIDENT_THUMBNAIL_CACHE_LIMIT: usize = 256;
const ANDROID_RESIDENT_THUMBNAIL_CACHE_LIMIT: usize = 72;
const ANDROID_DEVELOP_TEXTURE_CACHE_LIMIT: usize = 10;
const THUMBNAIL_PAUSE_POLL_INTERVAL: Duration = Duration::from_millis(20);
const THUMBNAIL_QUEUE_POLL_INTERVAL: Duration = Duration::from_millis(8);
const THUMBNAIL_RETRY_MAX_DELAY: Duration = Duration::from_secs(30);
/// The manual worker limit always allows at least this many on desktop, even
/// on machines with fewer cores.
pub(crate) const MIN_DESKTOP_THUMBNAIL_WORKER_LIMIT: usize = 8;
pub(crate) const MAX_DESKTOP_THUMBNAIL_WORKER_LIMIT: usize = 64;
pub(crate) const MAX_ANDROID_THUMBNAIL_WORKERS: usize = 2;
/// Workers of a media-library scan. At most the thumbnail-worker setting of
/// them read previews out of photos at once ([`ExtractionPermits`]); the rest
/// serve cached previews, so a screenful of them arrives together.
const MEDIA_LIBRARY_PREVIEW_WORKERS: usize = 4;
/// Concurrent full RAW renders for thumbnails of files without an embedded
/// preview, whatever the worker count. Each one holds an unpacked sensor
/// image, so this bounds the indexing memory peak.
const MAX_RENDERED_THUMBNAIL_WORKERS: usize = 4;
/// Peak memory one thumbnail job may need: a decoded full-size embedded JPEG
/// or a half-size RAW fallback of a high-resolution sensor.
const THUMBNAIL_WORKER_MEMORY_BUDGET_BYTES: u64 = 300 * 1024 * 1024;
/// Automatic workers together may claim this fraction of installed memory.
const THUMBNAIL_WORKER_MEMORY_SHARE_DIVISOR: u64 = 4;
const MAX_AUTOMATIC_THUMBNAIL_WORKERS: usize = 12;
/// The automatic count when installed memory is unknown.
const UNKNOWN_MEMORY_THUMBNAIL_WORKERS: usize = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LibrarySortOrder {
    #[default]
    NewestFirst,
    OldestFirst,
    NameAscending,
    NameDescending,
    LargestFirst,
    SmallestFirst,
    RatingHighestFirst,
    RatingLowestFirst,
    FlagPickedFirst,
    FlagRejectedFirst,
}

impl LibrarySortOrder {
    fn label(self) -> &'static str {
        match self {
            Self::NewestFirst => tr("Newest first"),
            Self::OldestFirst => tr("Oldest first"),
            Self::NameAscending => tr("Name A–Z"),
            Self::NameDescending => tr("Name Z–A"),
            Self::LargestFirst => tr("Largest first"),
            Self::SmallestFirst => tr("Smallest first"),
            Self::RatingHighestFirst => tr("Rating: highest first"),
            Self::RatingLowestFirst => tr("Rating: lowest first"),
            Self::FlagPickedFirst => tr("Flag: picks first"),
            Self::FlagRejectedFirst => tr("Flag: rejects first"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LibraryThumbnailSize {
    Small,
    Medium,
    #[default]
    Large,
    Enormous,
}

impl LibraryThumbnailSize {
    const ALL: [Self; 4] = [Self::Small, Self::Medium, Self::Large, Self::Enormous];

    fn label(self) -> &'static str {
        match self {
            Self::Small => tr("Small"),
            Self::Medium => tr("Medium"),
            Self::Large => tr("Large"),
            Self::Enormous => tr("Extra large"),
        }
    }

    const fn scale(self) -> f32 {
        match self {
            Self::Small => 1.0,
            Self::Medium => 1.25,
            Self::Large => 1.5,
            Self::Enormous => 1.75,
        }
    }

    /// Mouse-wheel speed of the thumbnail grid. Larger thumbnails scroll faster
    /// so one wheel notch still moves a comparable share of a row.
    const fn wheel_scroll_multiplier(self) -> f32 {
        match self {
            Self::Small => 1.25,
            Self::Medium => 1.5,
            Self::Large => 1.75,
            Self::Enormous => 2.0,
        }
    }
}

/// How the library shows the selected folder; each part persists with the
/// settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct LibraryView {
    pub(crate) thumbnail_size: LibraryThumbnailSize,
    /// Equal square tiles, each photo cropped to fill its tile, instead of
    /// justified rows of photos in their own shapes.
    pub(crate) square_thumbnails: bool,
    /// Also show the photos of every folder below the selected one.
    pub(crate) include_subfolders: bool,
}

/// The manual worker setting's initial and reset value.
pub(crate) fn default_thumbnail_worker_count() -> usize {
    sources::default_thumbnail_worker_count()
}

/// Workers used while the worker count is automatic.
pub(crate) fn automatic_thumbnail_worker_count() -> usize {
    sources::automatic_thumbnail_worker_count()
}

pub(crate) fn maximum_thumbnail_worker_count() -> usize {
    sources::maximum_thumbnail_worker_count()
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum LibraryAssetId {
    /// A photo in a folder, by path.
    Desktop(PathBuf),
    /// A photo in the media library, by content URI.
    Android(String),
}

impl From<&crate::sidecar::SidecarTarget> for LibraryAssetId {
    fn from(target: &crate::sidecar::SidecarTarget) -> Self {
        match target {
            crate::sidecar::SidecarTarget::Desktop { raw_path } => Self::Desktop(raw_path.clone()),
            crate::sidecar::SidecarTarget::Android { raw_uri, .. } => {
                Self::Android(raw_uri.clone())
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum LibraryLocator {
    Desktop(PathBuf),
    Android { uri: String },
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LibraryAssetMetadata {
    pub(crate) review: crate::sidecar::PhotoReview,
    /// Whether the saved edits differ from the photo's start
    /// ([`crate::sidecar::EditFingerprint`]).
    pub(crate) edited: bool,
    pub(crate) bytes: u64,
    pub(crate) dimensions_hint: Option<[u32; 2]>,
    /// Capture settings read from folder photos; zero for media-library photos.
    pub(crate) iso_speed: f32,
    pub(crate) shutter_seconds: f32,
    pub(crate) focal_length: f32,
    pub(crate) aperture: f32,
    pub(crate) modified_seconds: u64,
    /// When the photo was taken, in seconds since the Unix epoch: 0 until its
    /// capture settings have been read, or when the file carries no readable
    /// date. The newest/oldest library sorts order by this, falling back to
    /// `modified_seconds`; media-library documents stay on their file date.
    pub(crate) taken_seconds: u64,
}

impl LibraryAssetMetadata {
    /// What the newest/oldest sorts order this photo by: the date it was taken
    /// when that has been read, otherwise the file's own date. A photo can move
    /// in those orders once its capture date arrives, since a folder scan only
    /// knows file dates.
    pub(crate) fn date_order_seconds(&self) -> u64 {
        if self.taken_seconds == 0 {
            self.modified_seconds
        } else {
            self.taken_seconds
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct LibraryAsset {
    pub(crate) id: LibraryAssetId,
    pub(crate) display_name: String,
    pub(crate) display_path: String,
    pub(crate) locator: LibraryLocator,
    pub(crate) metadata: LibraryAssetMetadata,
}

impl LibraryAsset {
    pub(crate) fn from_desktop_path(
        path: PathBuf,
        bytes: u64,
        modified_seconds: u64,
        dimensions_hint: Option<[u32; 2]>,
    ) -> Self {
        let display_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let sidecar = crate::sidecar::load_sidecar_metadata(&path).unwrap_or_else(|error| {
            log::warn!("Could not read review for {}: {error}", path.display());
            crate::sidecar::SidecarMetadata::default()
        });
        Self {
            id: LibraryAssetId::Desktop(path.clone()),
            display_path: path.display().to_string(),
            display_name,
            locator: LibraryLocator::Desktop(path),
            metadata: LibraryAssetMetadata {
                review: sidecar.review,
                edited: sidecar.edited,
                bytes,
                dimensions_hint,
                iso_speed: 0.0,
                shutter_seconds: 0.0,
                focal_length: 0.0,
                aperture: 0.0,
                modified_seconds,
                taken_seconds: 0,
            },
        }
    }

    pub(crate) fn from_android_document(document: crate::platform::LibraryDocument) -> Self {
        Self {
            id: LibraryAssetId::Android(document.uri.clone()),
            display_name: document.display_name,
            display_path: document.display_path,
            locator: LibraryLocator::Android { uri: document.uri },
            metadata: LibraryAssetMetadata {
                review: crate::sidecar::PhotoReview::default(),
                edited: false,
                bytes: document.bytes,
                dimensions_hint: None,
                iso_speed: 0.0,
                shutter_seconds: 0.0,
                focal_length: 0.0,
                aperture: 0.0,
                modified_seconds: document.modified_seconds,
                taken_seconds: 0,
            },
        }
    }

    pub(crate) fn desktop_path(&self) -> Option<&Path> {
        match &self.locator {
            LibraryLocator::Desktop(path) => Some(path.as_path()),
            LibraryLocator::Android { .. } => None,
        }
    }

    pub(crate) fn android_uri(&self) -> Option<&str> {
        match &self.locator {
            LibraryLocator::Desktop(_) => None,
            LibraryLocator::Android { uri } => Some(uri.as_str()),
        }
    }
}

pub(crate) struct LibraryEntry {
    asset: LibraryAsset,
    texture: Option<egui::TextureHandle>,
    resident_thumbnail: Option<RawThumbnail>,
    texture_is_resident: bool,
    thumbnail_size: Option<[u32; 2]>,
    layout_size: Option<[u32; 2]>,
    thumbnail_error: Option<String>,
    thumbnail_failures: u8,
    thumbnail_retry_after: Option<Instant>,
    thumbnail_queued: bool,
    edits: ThumbnailEdits,
    last_used: u64,
}

impl LibraryEntry {
    fn review(&self) -> crate::sidecar::PhotoReview {
        self.asset.metadata.review
    }
}

/// What a tile's thumbnail shows of the photo's saved edits.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum ThumbnailEdits {
    /// The photo has no edits that change it, or no thumbnail yet.
    #[default]
    Unedited,
    /// A render of the saved edits.
    Developed,
    /// The camera's preview with the saved crop, or an earlier render, stands
    /// in until the saved edits render, once the previews have loaded.
    Rendering,
    /// As `Rendering`, but the edits do not render: the platform does not.
    Stale,
    /// Rendering the saved edits failed with this error.
    Failed(String),
}

impl ThumbnailEdits {
    /// Whether the thumbnail does not show the saved edits.
    fn is_stale(&self) -> bool {
        matches!(self, Self::Rendering | Self::Stale | Self::Failed(_))
    }
}

impl From<&crate::sidecar::SidecarSummary> for SidecarBadges {
    fn from(summary: &crate::sidecar::SidecarSummary) -> Self {
        Self {
            review: summary.review,
            edited: summary.edited,
        }
    }
}

/// A photo as Develop's filmstrip shows it, from folders or the media library.
#[derive(Clone)]
pub(crate) struct FilmstripItem {
    pub(crate) asset: LibraryAsset,
    pub(crate) texture: Option<egui::TextureHandle>,
    pub(crate) thumbnail_size: Option<[u32; 2]>,
    pub(crate) developed_thumbnail_pending: bool,
}

/// What a tile's badges show from the photo's sidecar.
#[derive(Clone, Copy, Debug)]
struct SidecarBadges {
    review: crate::sidecar::PhotoReview,
    /// Whether the edits differ from the photo's start.
    edited: bool,
}

struct LoadedLibraryThumbnail {
    thumbnail: RawThumbnail,
    resident_thumbnail: RawThumbnail,
    /// Read where loading the thumbnail reads the sidecar: in the media library.
    badges: Option<SidecarBadges>,
    edits: LoadedEdits,
    /// Display metadata the catalog scan deferred, read while loading this thumbnail.
    display_metadata: Option<crate::pipeline::RawDisplayMetadata>,
}

/// What a loaded thumbnail shows of the photo's saved edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoadedEdits {
    /// The photo has no edits that change it.
    Unedited,
    /// The developed thumbnail saved for the current edits.
    Developed,
    /// The camera's preview with the saved crop: the edits are not rendered.
    Stale,
}

#[derive(Clone)]
struct ThumbnailRequest {
    generation: u64,
    asset_id: LibraryAssetId,
    display_priority: bool,
}

struct ThumbnailWorkQueue {
    background: VecDeque<ThumbnailRequest>,
    in_flight: HashMap<LibraryAssetId, bool>,
    initial_completed: HashSet<LibraryAssetId>,
}

#[derive(Default)]
struct ThumbnailBackgroundProgress {
    generation: u64,
    total: usize,
    completed_assets: HashSet<LibraryAssetId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ThumbnailProgress {
    pub(crate) stage: ThumbnailStage,
    pub(crate) completed: usize,
    pub(crate) total: usize,
    pub(crate) paused: bool,
}

/// What the library's background work is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ThumbnailStage {
    /// Loading every photo's preview.
    Previews,
    /// Rendering the saved edits that previews do not show, after them.
    Edits,
}

impl ThumbnailBackgroundProgress {
    fn begin(&mut self, generation: u64, total: usize) {
        self.generation = generation;
        self.total = total;
        self.completed_assets.clear();
    }

    fn record_completion(&mut self, generation: u64, asset_id: LibraryAssetId) {
        if self.generation == generation {
            self.completed_assets.insert(asset_id);
        }
    }

    /// Whether every thumbnail of the folder has loaded or failed.
    fn is_complete(&self) -> bool {
        self.completed_assets.len() >= self.total
    }

    fn snapshot(&self, paused: bool) -> Option<ThumbnailProgress> {
        let completed = self.completed_assets.len().min(self.total);
        (self.total > 0 && completed < self.total).then_some(ThumbnailProgress {
            stage: ThumbnailStage::Previews,
            completed,
            total: self.total,
            paused,
        })
    }
}

impl ThumbnailWorkQueue {
    fn new(generation: u64, assets: &[LibraryAsset]) -> Self {
        Self {
            background: assets
                .iter()
                .map(|asset| ThumbnailRequest {
                    generation,
                    asset_id: asset.id.clone(),
                    display_priority: false,
                })
                .collect(),
            in_flight: HashMap::new(),
            initial_completed: HashSet::new(),
        }
    }

    fn claim(&mut self, request: &ThumbnailRequest, initial_background: bool) -> bool {
        if initial_background && self.initial_completed.contains(&request.asset_id) {
            return false;
        }
        if let Some(display_priority) = self.in_flight.get_mut(&request.asset_id) {
            *display_priority |= request.display_priority;
            return false;
        }
        self.in_flight
            .insert(request.asset_id.clone(), request.display_priority);
        true
    }

    fn finish(&mut self, request: &ThumbnailRequest) -> bool {
        self.initial_completed.insert(request.asset_id.clone());
        self.in_flight.remove(&request.asset_id).unwrap_or(false)
    }
}

enum ScanEvent {
    Catalog {
        generation: u64,
        assets: Vec<LibraryAsset>,
        warning_count: usize,
        truncated: bool,
    },
    FolderTree {
        generation: u64,
        tree: LibraryFolderNode,
    },
    AndroidFolders {
        generation: u64,
        folders: Vec<crate::platform::LibraryFolder>,
    },
    Thumbnail {
        generation: u64,
        asset_id: LibraryAssetId,
        display_priority: bool,
        result: Result<LoadedLibraryThumbnail, String>,
    },
    Failed {
        generation: u64,
        error: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImageClipboardMode {
    Copy,
    Cut,
}

#[derive(Clone, Debug)]
struct ImageClipboard {
    mode: ImageClipboardMode,
    assets: Vec<LibraryAsset>,
}

impl ImageClipboard {
    fn paste_label(&self) -> String {
        let count = self.assets.len();
        if count == 1 {
            tr_fmt("Paste {count} photo", &[("count", &count)])
        } else {
            tr_fmt("Paste {count} photos", &[("count", &count)])
        }
    }
}

#[derive(Clone, Debug)]
enum LibraryTransferDestination {
    LocalFolder(PathBuf),
    LocalLibrary,
}

struct AssetTransferCompletion {
    result: Result<String, String>,
    clear_clipboard: bool,
    remaining_clipboard: Option<ImageClipboard>,
}

struct ThumbnailWorker {
    assets: Vec<LibraryAsset>,
    warning_count: usize,
    truncated: bool,
    generation: u64,
    cancellation: Arc<AtomicU64>,
    decoding_paused: Arc<AtomicBool>,
    decode_gate: Arc<RwLock<()>>,
    event_sender: mpsc::SyncSender<ScanEvent>,
    request_receiver: mpsc::Receiver<ThumbnailRequest>,
    repaint: egui::Context,
}

#[derive(Clone)]
struct LibraryExportDialog {
    assets: Vec<LibraryAsset>,
    settings: ExportSettings,
    format: ExportFormat,
    overrides: crate::app::ExportOverrideChoice,
}

/// Choosing which settings of `asset` Copy adjustments takes.
struct LibraryAdjustmentCopyDialog {
    asset: LibraryAsset,
    /// The photo's edits when the dialog opened. The dialog is modal, so they
    /// are what Copy installs.
    edits: Box<crate::sidecar::EditState>,
    selection: crate::sidecar::EditSelection,
}

#[derive(Clone)]
struct LibraryAdjustmentPasteDialog {
    assets: Vec<LibraryAsset>,
    edited_count: usize,
}

#[derive(Clone)]
struct LibraryAiMaskRefreshPrompt {
    assets: Vec<LibraryAsset>,
}

struct RawImportResult {
    imported: Vec<PathBuf>,
    imported_folders: Vec<PathBuf>,
    already_present: usize,
    ignored: usize,
    failures: Vec<String>,
}

enum RawImportOutcome {
    Imported(PathBuf),
    AlreadyPresent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LibraryFolderClipboardMode {
    Copy,
    Cut,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LibraryFolderClipboard {
    path: PathBuf,
    mode: LibraryFolderClipboardMode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LibraryFolderDrag(PathBuf);

enum LibraryFolderOperation {
    Create {
        root: PathBuf,
        parent: PathBuf,
        name: String,
    },
    Copy {
        root: PathBuf,
        source: PathBuf,
        destination_parent: PathBuf,
    },
    Move {
        root: PathBuf,
        source: PathBuf,
        destination_parent: PathBuf,
        new_name: Option<String>,
    },
    Delete {
        root: PathBuf,
        target: PathBuf,
    },
}

enum LibraryFolderOperationResult {
    Created(PathBuf),
    Copied {
        source: PathBuf,
        destination: PathBuf,
    },
    Moved {
        source: PathBuf,
        destination: PathBuf,
    },
    Deleted(PathBuf),
}

struct LibraryFolderOperationCompletion {
    root: PathBuf,
    result: Result<LibraryFolderOperationResult, String>,
}

enum LibraryFolderUiAction {
    New(PathBuf),
    Copy(PathBuf),
    Cut(PathBuf),
    Paste(PathBuf),
    PasteImages(PathBuf),
    Rename(PathBuf),
    Delete(PathBuf),
    Move {
        source: PathBuf,
        destination_parent: PathBuf,
    },
    Refresh,
}

enum LibraryFolderNameDialogKind {
    Create { parent: PathBuf },
    Rename { source: PathBuf },
}

struct LibraryFolderNameDialog {
    kind: LibraryFolderNameDialogKind,
    name: String,
    error: Option<String>,
    focus_requested: bool,
}

struct LibraryRawNameDialog {
    asset: LibraryAsset,
    name: String,
    error: Option<String>,
    focus_requested: bool,
}

struct AndroidLibraryFolderNameDialog {
    parent: String,
    name: String,
    error: Option<String>,
    focus_requested: bool,
}

/// The media library's folders; empty where storage is folders.
#[derive(Default)]
struct MediaLibraryState {
    root_location: String,
    folder: String,
    /// The folders of every root the sidebar shows, by root key (empty for
    /// the media library), kept while another root is shown.
    trees: HashMap<String, FolderTree>,
    folder_name_dialog: Option<AndroidLibraryFolderNameDialog>,
    /// The linked folder shown; empty for the media library itself.
    root: String,
    linked_folders: Vec<crate::platform::LinkedFolder>,
    /// Linked folders whose last scan failed.
    unavailable_roots: HashSet<String>,
    link_dialog: bool,
    unlink_dialog: Option<crate::platform::LinkedFolder>,
    /// A failure the app shows in its error dialog on the next frame.
    pending_error: Option<String>,
    /// The walk of one root's tree. A linked folder's lists every file of
    /// every folder through the Storage Access Framework, so one runs at a
    /// time and selecting a folder within the tree does not start one.
    tree_walk: Option<FolderTreeWalk>,
    /// A refresh asked for the shown linked folder's tree again; walked once
    /// no walk runs.
    tree_walk_due: bool,
}

impl MediaLibraryState {
    /// The tree of the root shown.
    fn shown_tree_mut(&mut self) -> &mut FolderTree {
        self.trees.entry(self.root.clone()).or_default()
    }

    /// Whether `key` still names a root: the media library or a linked folder.
    fn has_root(&self, key: &str) -> bool {
        key.is_empty() || self.linked_folders.iter().any(|folder| folder.key == key)
    }
}

#[derive(Default)]
struct FolderTree {
    folders: Vec<crate::platform::LibraryFolder>,
    /// Open rows by folder path; the empty path is the root's own row.
    expanded: HashSet<String>,
    /// Set once the folders were listed, even when that failed, so a root that
    /// is not shown is walked once rather than on every frame.
    listed: bool,
}

struct FolderTreeWalk {
    root: String,
    result: mpsc::Receiver<Result<Vec<crate::platform::LibraryFolder>, String>>,
}

enum AndroidLibraryFolderUiAction {
    /// A folder of a root, which need not be the root shown.
    Select {
        root: String,
        folder: String,
    },
    New(String),
    Refresh,
    Link,
    Unlink(crate::platform::LinkedFolder),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LibraryFolderNode {
    path: PathBuf,
    name: String,
    children: Vec<Self>,
}

impl LibraryFolderNode {
    fn empty(path: PathBuf) -> Self {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| path.display().to_string());
        Self {
            path,
            name,
            children: Vec::new(),
        }
    }
}

/// Adjustments taken with Copy adjustments, waiting to be pasted.
#[derive(Clone, Debug)]
pub(crate) struct AdjustmentClipboard {
    pub(crate) edits: crate::sidecar::EditState,
    /// The categories chosen in Settings, or in the copy dialog, when the
    /// adjustments were copied.
    pub(crate) selection: crate::sidecar::EditSelection,
}

pub(crate) struct LibraryState {
    location: Option<String>,
    folder: Option<PathBuf>,
    root_folder: Option<PathBuf>,
    folder_tree: Option<LibraryFolderNode>,
    expanded_folders: HashSet<PathBuf>,
    folder_sidebar_open: bool,
    platform: Arc<dyn Platform>,
    media: MediaLibraryState,
    entries: Vec<LibraryEntry>,
    entry_indices: HashMap<LibraryAssetId, usize>,
    event_receiver: Option<mpsc::Receiver<ScanEvent>>,
    request_sender: Option<mpsc::SyncSender<ThumbnailRequest>>,
    generation: Arc<AtomicU64>,
    decoding_paused: Arc<AtomicBool>,
    decode_gate: Arc<RwLock<()>>,
    thumbnail_progress: ThumbnailBackgroundProgress,
    /// Renders the saved edits of photos whose thumbnail does not show them.
    developed: DevelopedRenders,
    /// The editor's GPU device for those renders, once the app has one.
    saved_edits_gpu: Option<crate::app::SavedEditsGpu>,
    scanning: bool,
    catalog_ready: bool,
    status: String,
    usage_clock: u64,
    /// The manual worker limit; see `automatic_thumbnail_workers`.
    thumbnail_workers: usize,
    automatic_thumbnail_workers: bool,
    sort_order: LibrarySortOrder,
    view: LibraryView,
    search_query: String,
    review_filter: LibraryReviewFilter,
    /// Show RAW+JPEG pairs once, as the RAW. Rebuilt with the entry indices.
    stack_raw_companions: bool,
    raw_companions: RawCompanions,
    selection: state::LibrarySelection,
    reviews: state::ReviewWriter,
    image_clipboard: Option<ImageClipboard>,
    pub(crate) adjustment_clipboard: Option<AdjustmentClipboard>,
    /// The last selection confirmed in the copy dialog, which opens with it
    /// again until CalibRaw quits.
    chosen_copy_selection: Option<crate::sidecar::EditSelection>,
    asset_transfer_receiver: Option<mpsc::Receiver<AssetTransferCompletion>>,
    merge: Option<merge::MergeTask>,
    raw_import_receiver: Option<mpsc::Receiver<RawImportResult>>,
    folder_operation_receiver: Option<mpsc::Receiver<LibraryFolderOperationCompletion>>,
    folder_clipboard: Option<LibraryFolderClipboard>,
    folder_name_dialog: Option<LibraryFolderNameDialog>,
    folder_delete_confirmation: Option<PathBuf>,
    delete_originals_confirmation: Option<Vec<LibraryAsset>>,
    raw_name_dialog: Option<LibraryRawNameDialog>,
    export_dialog: Option<LibraryExportDialog>,
    adjustment_copy_dialog: Option<LibraryAdjustmentCopyDialog>,
    adjustment_paste_dialog: Option<LibraryAdjustmentPasteDialog>,
    ai_mask_refresh_prompt: Option<LibraryAiMaskRefreshPrompt>,
}

#[cfg(test)]
mod tests;
