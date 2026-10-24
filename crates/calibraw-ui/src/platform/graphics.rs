//! What CalibRaw needs from this OS's graphics driver, for the notice shown
//! when the driver is unsupported.

/// The graphics APIs CalibRaw processes photos with on this OS.
pub(crate) const PROCESSING_GRAPHICS_API: &str = if cfg!(target_os = "windows") {
    "Direct3D 12 or Vulkan"
} else if cfg!(any(target_os = "macos", target_os = "ios")) {
    "Metal"
} else {
    "Vulkan"
};

/// How to add a driver for [`PROCESSING_GRAPHICS_API`] where only an
/// unsupported one was found, if the user can do that.
pub(crate) const PROCESSING_DRIVER_INSTALL_HINT: Option<&str> = if cfg!(target_os = "linux") {
    Some(
        "Installing Mesa's software Vulkan driver, lavapipe (package mesa-vulkan-drivers or vulkan-swrast), lets CalibRaw run on the CPU.",
    )
} else {
    None
};
