//! Edit In: the photo rendered to a file and opened in another editor. Nothing
//! comes back into CalibRaw; the file is the other editor's to save over or
//! save elsewhere.
//!
//! The serialized names here are stored in the settings file and must not
//! change.

use crate::export_folders::EditInFolder;
use crate::i18n::tr;
use crate::pipeline::{
    ExportBitDepth, ExportFormat, ExportResizeMode, ExportSettings, OutputColorSpace,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// An editor the person added in Settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ExternalEditor {
    pub(crate) name: String,
    /// What opens the file: a Linux `.desktop` entry, a macOS `.app` bundle
    /// or a program; `Platform::open_in_editor` tells them apart.
    pub(crate) program: PathBuf,
    /// Hand over the photo's own file instead of a rendered copy, for an
    /// editor that develops RAWs itself.
    #[serde(default)]
    pub(crate) send_original: bool,
}

/// The file Edit In renders.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum EditInFormat {
    #[default]
    Tiff16,
    Tiff8,
    Jpeg,
}

impl EditInFormat {
    pub(crate) const ALL: [Self; 3] = [Self::Tiff16, Self::Tiff8, Self::Jpeg];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Tiff16 => tr("16-bit TIFF"),
            Self::Tiff8 => tr("8-bit TIFF"),
            Self::Jpeg => "JPEG",
        }
    }

    pub(crate) const fn export_format(self) -> ExportFormat {
        match self {
            Self::Tiff16 | Self::Tiff8 => ExportFormat::Tiff,
            Self::Jpeg => ExportFormat::Jpeg,
        }
    }

    const fn bit_depth(self) -> ExportBitDepth {
        match self {
            Self::Tiff16 => ExportBitDepth::Sixteen,
            Self::Tiff8 | Self::Jpeg => ExportBitDepth::Eight,
        }
    }
}

/// How Edit In renders and where it saves; app settings of folder storage.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EditInSettings {
    pub(crate) folder: EditInFolder,
    pub(crate) format: EditInFormat,
    pub(crate) color_space: OutputColorSpace,
    /// In the order the menus list them.
    pub(crate) editors: Vec<ExternalEditor>,
}

impl EditInSettings {
    /// What the handoff is rendered with: the whole photo at full size, with
    /// its metadata and without the export watermark, which marks a finished
    /// image rather than one handed on for more editing.
    pub(crate) fn export_settings(&self) -> ExportSettings {
        ExportSettings {
            resize_mode: ExportResizeMode::Original,
            keep_metadata: true,
            // Above the export default of 90: the other editor saves the
            // image again, so it starts with as little loss as JPEG allows
            // without the size of quality 100.
            jpeg_quality: 95,
            bit_depth: self.format.bit_depth(),
            color_space: self.color_space,
            watermark: None,
            ..ExportSettings::default()
        }
    }
}

/// The editor an Edit In opens its file in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EditInTarget {
    Editor(ExternalEditor),
    /// The system asks which application opens it.
    SystemChooser,
}

impl EditInTarget {
    pub(crate) fn sends_original(&self) -> bool {
        matches!(self, Self::Editor(editor) if editor.send_original)
    }
}
