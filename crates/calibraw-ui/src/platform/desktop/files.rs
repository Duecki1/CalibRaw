//! Native dialogs (`rfd`), the trash and DCP profile folders.

use super::super::files::FileDialog;
use std::path::{Path, PathBuf};

pub(in crate::platform) use super::portal::is_flatpak;

fn async_dialog(dialog: FileDialog) -> rfd::AsyncFileDialog {
    let mut native = rfd::AsyncFileDialog::new();
    for (name, extensions) in &dialog.filters {
        native = native.add_filter(name.clone(), extensions);
    }
    if let Some(title) = dialog.title {
        native = native.set_title(title);
    }
    if let Some(directory) = dialog.directory {
        native = native.set_directory(directory);
    }
    if let Some(file_name) = dialog.file_name {
        native = native.set_file_name(file_name);
    }
    native
}

fn sync_dialog(dialog: FileDialog) -> rfd::FileDialog {
    let mut native = rfd::FileDialog::new();
    for (name, extensions) in &dialog.filters {
        native = native.add_filter(name.clone(), extensions);
    }
    if let Some(title) = dialog.title {
        native = native.set_title(title);
    }
    if let Some(directory) = dialog.directory {
        native = native.set_directory(directory);
    }
    if let Some(file_name) = dialog.file_name {
        native = native.set_file_name(file_name);
    }
    native
}

pub(in crate::platform) fn pick_file_on_worker(dialog: FileDialog) -> Option<PathBuf> {
    pollster::block_on(async_dialog(dialog).pick_file()).map(|handle| handle.path().to_path_buf())
}

pub(in crate::platform) fn pick_files_on_worker(dialog: FileDialog) -> Option<Vec<PathBuf>> {
    pollster::block_on(async_dialog(dialog).pick_files()).map(|handles| {
        handles
            .into_iter()
            .map(|handle| handle.path().to_path_buf())
            .collect()
    })
}

pub(in crate::platform) fn pick_folder_on_worker(dialog: FileDialog) -> Option<PathBuf> {
    pollster::block_on(async_dialog(dialog).pick_folder()).map(|handle| handle.path().to_path_buf())
}

pub(in crate::platform) fn pick_folder(dialog: FileDialog) -> Option<PathBuf> {
    sync_dialog(dialog).pick_folder()
}

pub(in crate::platform) fn save_file(dialog: FileDialog) -> Option<PathBuf> {
    sync_dialog(dialog).save_file()
}

pub(in crate::platform) fn trash(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    if is_flatpak() {
        return super::portal::trash_file(path).map_err(|error| format!("{error:#}"));
    }
    trash::delete(path).map_err(|error| error.to_string())
}

/// Opens File Explorer with `path` selected.
#[cfg(target_os = "windows")]
pub(in crate::platform) fn reveal_file(path: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;

    // Explorer parses its own command line and needs the switch outside the
    // quotes. `Command::arg` would quote the whole token once the path has a
    // space, and Explorer then opens Documents without reporting anything.
    // A Windows path cannot contain `"`, so it needs no escaping here.
    let mut argument = std::ffi::OsString::from("/select,\"");
    argument.push(path);
    argument.push("\"");
    std::process::Command::new("explorer.exe")
        .raw_arg(argument)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Opens Finder with `path` selected.
#[cfg(target_os = "macos")]
pub(in crate::platform) fn reveal_file(path: &Path) -> Result<(), String> {
    std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// How long the UI thread waits for the file manager request to finish. A
/// missing service fails at once, so this still reports it, while starting a
/// file manager can take seconds and must not freeze the app.
#[cfg(target_os = "linux")]
const REVEAL_WAIT: std::time::Duration = std::time::Duration::from_millis(200);

/// Opens the file manager with `path` selected, or at least its folder when the
/// desktop offers no way to select it. A failure later than [`REVEAL_WAIT`] is
/// only logged.
#[cfg(target_os = "linux")]
pub(in crate::platform) fn reveal_file(path: &Path) -> Result<(), String> {
    let path = path.to_path_buf();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("calibraw-reveal-file".to_owned())
        .spawn(move || {
            let result = select_in_file_manager(&path).or_else(|error| {
                log::info!(
                    "Opening the folder of {} instead of selecting it: {error:#}",
                    path.display()
                );
                open_parent_folder(&path)
            });
            if let Err(error) = &result {
                log::error!("Could not show {}: {error}", path.display());
            }
            let _ = sender.send(result);
        })
        .map_err(|error| error.to_string())?;
    receiver.recv_timeout(REVEAL_WAIT).unwrap_or(Ok(()))
}

#[cfg(target_os = "linux")]
fn select_in_file_manager(path: &Path) -> anyhow::Result<()> {
    // Flatpak's bus filter hides FileManager1; the portal calls it for us.
    if is_flatpak() {
        return super::portal::open_directory(path);
    }
    let uri = file_uri(&std::path::absolute(path)?);
    zbus::blocking::Connection::session()?.call_method(
        Some("org.freedesktop.FileManager1"),
        "/org/freedesktop/FileManager1",
        Some("org.freedesktop.FileManager1"),
        "ShowItems",
        &(vec![uri.as_str()], ""),
    )?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn open_parent_folder(path: &Path) -> Result<(), String> {
    let status = std::process::Command::new("xdg-open")
        .arg(path.parent().unwrap_or(path))
        .status()
        .map_err(|error| format!("xdg-open: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("xdg-open: {status}"))
    }
}

/// The `file://` URI of an absolute path, percent-encoding its raw bytes.
#[cfg(target_os = "linux")]
fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;

    let mut uri = String::from("file://");
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-_.~".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

pub(in crate::platform) fn show_fatal_message(title: &str, description: &str) {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title(title)
        .set_description(description)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}

/// Human-readable form of [`camera_profile_candidates`], shown during setup;
/// keep the two in sync.
#[cfg(target_os = "windows")]
pub(in crate::platform) const CAMERA_PROFILE_SEARCH_LOCATIONS: &[&str] = &[
    r"%ProgramData%\CameraRaw\CameraProfiles",
    r"%ProgramData%\<vendor>\CameraRaw\CameraProfiles",
];
#[cfg(target_os = "macos")]
pub(in crate::platform) const CAMERA_PROFILE_SEARCH_LOCATIONS: &[&str] = &[
    "~/Library/Application Support/<vendor>/CameraRaw/CameraProfiles",
    "/Library/Application Support/<vendor>/CameraRaw/CameraProfiles",
];
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub(in crate::platform) const CAMERA_PROFILE_SEARCH_LOCATIONS: &[&str] = &[];

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

pub(in crate::platform) fn camera_profile_candidates() -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        if let Some(program_data) =
            std::env::var_os("ProgramData").or_else(|| std::env::var_os("ALLUSERSPROFILE"))
        {
            camera_profile_candidates_under(&PathBuf::from(program_data))
        } else {
            Vec::new()
        }
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
        candidates
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Vec::new()
    }

    #[cfg(not(any(windows, unix, target_os = "macos")))]
    {
        Vec::new()
    }
}
