//! Where export dialogs open and where Edit In saves the copy it hands to
//! another editor. Both are app settings of folder storage; a media library
//! chooses its own destinations.
//!
//! The serialized names (`photo-folder`, `app-cache`, `{"folder": path}`) are
//! stored in the settings file and must not change.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The folder export save dialogs open in.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ExportFolder {
    /// The folder of the photo being exported.
    #[default]
    PhotoFolder,
    /// A folder the person chose.
    Folder(PathBuf),
}

impl ExportFolder {
    /// The folder a save dialog for the photo at `photo` opens in. A chosen
    /// folder that is no longer there (an unplugged drive, a renamed folder)
    /// falls back to the photo's folder, so the dialog never opens somewhere
    /// the platform picks instead.
    pub(crate) fn start_directory<'a>(&'a self, photo: Option<&'a Path>) -> Option<&'a Path> {
        match self {
            Self::Folder(folder) if folder.is_dir() => Some(folder),
            Self::Folder(_) | Self::PhotoFolder => photo
                .and_then(Path::parent)
                .filter(|parent| !parent.as_os_str().is_empty()),
        }
    }
}

/// Where Edit In saves the image it opens in another editor.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum EditInFolder {
    /// A folder in the app's cache, so copies do not collect beside the photos.
    #[default]
    AppCache,
    /// The folder of the photo being edited.
    PhotoFolder,
    /// A folder the person chose.
    Folder(PathBuf),
}
