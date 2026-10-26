/// What the device offers. Views and handlers branch on these instead of
/// naming an operating system, so every branch compiles and can be tested on
/// any host.
///
/// [`PlatformCaps::CURRENT`] describes the build target; tests use
/// [`PlatformCaps::DESKTOP`] or [`PlatformCaps::ANDROID`] to exercise the other
/// platform's paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PlatformCaps {
    pub(crate) form_factor: FormFactor,
    pub(crate) storage: StorageKind,
    pub(crate) memory: MemoryClass,
    /// Long tasks stop when the app leaves the foreground, so they hold the
    /// UI and show a system notification while they run.
    pub(crate) tasks_need_foreground: bool,
    /// The person chooses the ONNX Runtime (automatic download or a manual
    /// library) and whether AI may use the GPU. Elsewhere the runtime is
    /// built into the app and GPU AI is always allowed.
    pub(crate) configurable_ai_runtime: bool,
    pub(crate) video_encoder: VideoEncoder,
    /// The app can start itself again, so a lost GPU device ends in a
    /// native notice and a restart.
    pub(crate) can_restart: bool,
}

/// How the person works with the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FormFactor {
    /// Resizable windows, pointer and keyboard: menus, hover, shortcuts,
    /// resizable panels and the filmstrip.
    Desktop,
    /// One full-screen touch surface with system bars and a back gesture.
    Mobile,
}

/// Where photos and their edits live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StorageKind {
    /// Folders on a file system the app reads and writes by path, with edit
    /// sidecars beside the photos and native file dialogs.
    Folders,
    /// The app's media library, reached through content URIs; imports,
    /// exports and sidecars go through the platform bridge.
    MediaLibrary,
}

/// What encodes the Edit Replay video.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VideoEncoder {
    /// An FFmpeg program with libx264, found on `PATH` or through
    /// `CALIBRAW_FFMPEG`.
    Ffmpeg,
    /// The platform's own codec, through [`super::Platform::start_replay_encoder`].
    Platform,
}

/// How much memory the app can count on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MemoryClass {
    /// A desktop computer: caches and worker pools sized from installed
    /// memory and cores.
    Standard,
    /// A phone or tablet, where memory is shared with the GPU and the system
    /// ends apps under pressure: small fixed caches and pools.
    Constrained,
}

impl PlatformCaps {
    pub(crate) const DESKTOP: Self = Self {
        form_factor: FormFactor::Desktop,
        storage: StorageKind::Folders,
        memory: MemoryClass::Standard,
        tasks_need_foreground: false,
        configurable_ai_runtime: true,
        video_encoder: VideoEncoder::Ffmpeg,
        can_restart: true,
    };

    pub(crate) const ANDROID: Self = Self {
        form_factor: FormFactor::Mobile,
        storage: StorageKind::MediaLibrary,
        memory: MemoryClass::Constrained,
        tasks_need_foreground: true,
        configurable_ai_runtime: false,
        video_encoder: VideoEncoder::Platform,
        can_restart: false,
    };

    #[cfg(target_os = "ios")]
    pub(crate) const IOS: Self = Self {
        form_factor: FormFactor::Mobile,
        storage: StorageKind::MediaLibrary,
        memory: MemoryClass::Constrained,
        tasks_need_foreground: false,
        configurable_ai_runtime: false,
        video_encoder: VideoEncoder::Platform,
        can_restart: false,
    };

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub(crate) const CURRENT: Self = Self::DESKTOP;
    #[cfg(target_os = "android")]
    pub(crate) const CURRENT: Self = Self::ANDROID;
    #[cfg(target_os = "ios")]
    pub(crate) const CURRENT: Self = Self::IOS;

    pub(crate) const fn is_desktop(self) -> bool {
        matches!(self.form_factor, FormFactor::Desktop)
    }

    pub(crate) const fn is_mobile(self) -> bool {
        matches!(self.form_factor, FormFactor::Mobile)
    }

    pub(crate) const fn has_folders(self) -> bool {
        matches!(self.storage, StorageKind::Folders)
    }

    pub(crate) const fn has_media_library(self) -> bool {
        matches!(self.storage, StorageKind::MediaLibrary)
    }

    pub(crate) const fn constrained_memory(self) -> bool {
        matches!(self.memory, MemoryClass::Constrained)
    }

    /// Whether the preview can be rendered from the whole native photo while
    /// idle: it takes a graph of the photo's size for a few seconds, which
    /// only a device with its own GPU memory has.
    pub(crate) const fn full_resolution_preview(self) -> bool {
        !self.constrained_memory()
    }
}
