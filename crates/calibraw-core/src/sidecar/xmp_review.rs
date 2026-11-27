//! Ratings and flags in XMP, the metadata other photo apps read and write,
//! kept alongside the review in a photo's CalibRaw sidecar.
//!
//! [`XmpReviewSync`] decides how the two relate. CalibRaw's sidecar keeps the
//! review in every mode, so turning XMP off loses nothing. The settings are
//! process-wide ([`set_xmp_review_settings`]), as every path that reads or
//! saves a sidecar must apply them alike: the library, Develop, pasting and
//! resetting edits, and the rating saver.
//!
//! [`XmpFiles`] is where a photo's XMP is kept. Desktop reaches it by path
//! ([`resolve_photo_review`]); the Android library and its linked folders
//! through the platform's storage, which calibraw-ffi implements it with.

mod embedded;
mod mapping;

use super::{atomic_write, xmp_sidecar_path_for_raw, PhotoReview, SidecarError};
pub use mapping::export_packet;
use mapping::XmpReview;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{PoisonError, RwLock};

/// Largest XMP file or embedded packet read. Lightroom sidecars that hold
/// masks reach a few megabytes. Android's copy is `LibraryStore.MAX_XMP_BYTES`;
/// change both together.
const MAX_XMP_BYTES: u64 = 64 * 1024 * 1024;

/// How CalibRaw uses the ratings in XMP files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum XmpReviewSync {
    /// XMP is neither read nor written.
    #[default]
    Off,
    /// A photo without a CalibRaw sidecar shows the review its XMP records,
    /// which its first sidecar keeps. Reviews are saved to CalibRaw's
    /// sidecar only.
    Read,
    /// What XMP records wins over CalibRaw's sidecar, and every review change
    /// is saved to XMP as well.
    ReadWrite,
}

impl XmpReviewSync {
    /// Whether review changes are saved to XMP.
    pub const fn writes(self) -> bool {
        matches!(self, Self::ReadWrite)
    }
}

/// How an XMP sidecar is named.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum XmpSidecarName {
    /// `IMG_0001.CR3.xmp`, as darktable and digiKam name it: one per photo.
    #[default]
    WithExtension,
    /// `IMG_0001.xmp`, as Lightroom and Capture One name it. A RAW and a
    /// JPEG with the same name share it.
    Basename,
}

impl XmpSidecarName {
    const fn other(self) -> Self {
        match self {
            Self::WithExtension => Self::Basename,
            Self::Basename => Self::WithExtension,
        }
    }

    fn path(self, raw_path: &Path) -> PathBuf {
        match self {
            Self::WithExtension => xmp_sidecar_path_for_raw(raw_path),
            Self::Basename => raw_path.with_extension("xmp"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct XmpReviewSettings {
    pub sync: XmpReviewSync,
    /// The name looked for first, and given to a new sidecar.
    pub sidecar_name: XmpSidecarName,
}

static SETTINGS: RwLock<XmpReviewSettings> = RwLock::new(XmpReviewSettings {
    sync: XmpReviewSync::Off,
    sidecar_name: XmpSidecarName::WithExtension,
});

/// Applies `settings` to every desktop sidecar read and saved from now on.
pub fn set_xmp_review_settings(settings: XmpReviewSettings) {
    // A plain value: a panic while it was held cannot leave it half-written.
    *SETTINGS.write().unwrap_or_else(PoisonError::into_inner) = settings;
}

fn xmp_review_settings() -> XmpReviewSettings {
    *SETTINGS.read().unwrap_or_else(PoisonError::into_inner)
}

/// Where a photo's XMP is kept: the sidecars beside it, by name, and the
/// packet embedded in it.
pub trait XmpFiles {
    /// The sidecar called `name`, for messages.
    fn sidecar_location(&self, name: XmpSidecarName) -> String;
    /// The photo, for messages.
    fn photo_location(&self) -> String;
    /// The sidecar called `name`, read as [`read_xmp_file`] reads one; `None`
    /// when there is none.
    fn read_sidecar(&self, name: XmpSidecarName) -> Result<Option<Vec<u8>>, SidecarError>;
    /// Replaces the sidecar called `name` with `bytes`, or creates it, as
    /// atomically as the storage allows.
    fn write_sidecar(&self, name: XmpSidecarName, bytes: &[u8]) -> Result<(), SidecarError>;
    /// The packet embedded in the photo, as [`embedded_packet`] reads it.
    fn embedded_packet(&self) -> Result<Option<Vec<u8>>, SidecarError>;
}

/// The XMP of the photo at a desktop path.
struct DesktopXmpFiles<'a>(&'a Path);

impl XmpFiles for DesktopXmpFiles<'_> {
    fn sidecar_location(&self, name: XmpSidecarName) -> String {
        name.path(self.0).display().to_string()
    }

    fn photo_location(&self) -> String {
        self.0.display().to_string()
    }

    fn read_sidecar(&self, name: XmpSidecarName) -> Result<Option<Vec<u8>>, SidecarError> {
        read_xmp_file(&name.path(self.0))
    }

    fn write_sidecar(&self, name: XmpSidecarName, bytes: &[u8]) -> Result<(), SidecarError> {
        atomic_write(&name.path(self.0), bytes)
    }

    fn embedded_packet(&self) -> Result<Option<Vec<u8>>, SidecarError> {
        embedded_packet(self.0)
    }
}

/// The review of the photo at `raw_path`, from the review in its CalibRaw
/// sidecar (`None` when it has no sidecar) and, as the settings say, its XMP.
pub fn resolve_photo_review(raw_path: &Path, sidecar_review: Option<PhotoReview>) -> PhotoReview {
    resolve_review(&DesktopXmpFiles(raw_path), sidecar_review)
}

/// [`resolve_photo_review`] for a photo whose XMP `files` reaches.
pub fn resolve_review(files: &dyn XmpFiles, sidecar_review: Option<PhotoReview>) -> PhotoReview {
    let settings = xmp_review_settings();
    let base = sidecar_review.unwrap_or_default();
    let reads_xmp = match settings.sync {
        XmpReviewSync::Off => false,
        XmpReviewSync::Read => sidecar_review.is_none(),
        XmpReviewSync::ReadWrite => true,
    };
    if !reads_xmp {
        return base;
    }
    read_xmp_review(files, settings.sidecar_name).over(base)
}

/// The review the photo's XMP records: from the first of its XMP sidecars
/// that records one, `preferred` name first, else from the packet embedded in
/// the photo file. An unreadable file is logged and passed over.
fn read_xmp_review(files: &dyn XmpFiles, preferred: XmpSidecarName) -> XmpReview {
    for name in sidecar_names(preferred) {
        match files
            .read_sidecar(name)
            .and_then(|bytes| bytes.map(|bytes| mapping::read_review(&bytes)).transpose())
        {
            Ok(Some(review)) if !review.is_empty() => return review,
            Ok(_) => {}
            Err(error) => log::warn!(
                "could not read the rating in {}: {error}",
                files.sidecar_location(name)
            ),
        }
    }
    match files.embedded_packet() {
        Ok(Some(bytes)) => mapping::read_review(&bytes).unwrap_or_else(|error| {
            log::warn!(
                "could not read the rating in {}: {error}",
                files.photo_location()
            );
            XmpReview::default()
        }),
        Ok(None) => XmpReview::default(),
        Err(error) => {
            log::warn!(
                "could not read the XMP in {}: {error}",
                files.photo_location()
            );
            XmpReview::default()
        }
    }
}

/// [`save_review`] for the photo at a desktop path.
pub(super) fn save_photo_xmp_review(
    raw_path: &Path,
    review: PhotoReview,
) -> Result<(), SidecarError> {
    save_review(&DesktopXmpFiles(raw_path), review)
}

/// Saves `review` to the photo's XMP sidecar when the settings write XMP,
/// and does nothing otherwise: to the first sidecar that records a review,
/// the preferred name first, else the first that exists, else a new sidecar
/// with the preferred name. Every other byte of an existing sidecar is kept;
/// one CalibRaw cannot read is left alone and reported.
pub fn save_review(files: &dyn XmpFiles, review: PhotoReview) -> Result<(), SidecarError> {
    let settings = xmp_review_settings();
    if !settings.sync.writes() {
        return Ok(());
    }
    let preferred = settings.sidecar_name;
    let mut existing = Vec::new();
    for name in sidecar_names(preferred) {
        if let Some(bytes) = files
            .read_sidecar(name)
            .map_err(|error| in_file(&files.sidecar_location(name), error))?
        {
            existing.push((name, bytes));
        }
    }
    let records_review =
        |bytes: &[u8]| mapping::read_review(bytes).is_ok_and(|found| !found.is_empty());
    let target = existing
        .iter()
        .position(|(_, bytes)| records_review(bytes))
        .or((!existing.is_empty()).then_some(0));
    let (name, bytes) = match target.map(|index| &existing[index]) {
        Some((name, bytes)) => (
            *name,
            mapping::write_review(bytes, review)
                .map_err(|error| in_file(&files.sidecar_location(*name), error))?,
        ),
        None => (preferred, Some(mapping::new_sidecar(review).into_bytes())),
    };
    match bytes {
        Some(bytes) => files
            .write_sidecar(name, &bytes)
            .map_err(|error| in_file(&files.sidecar_location(name), error)),
        None => Ok(()),
    }
}

/// The photo's XMP sidecar names, `preferred` first.
const fn sidecar_names(preferred: XmpSidecarName) -> [XmpSidecarName; 2] {
    [preferred, preferred.other()]
}

/// The XMP file at `path`, at most [`MAX_XMP_BYTES`]; `None` when there is
/// none.
pub fn read_xmp_file(path: &Path) -> Result<Option<Vec<u8>>, SidecarError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_XMP_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_XMP_BYTES {
        return Err(SidecarError::Invalid(format!(
            "the XMP file is larger than the {MAX_XMP_BYTES}-byte limit"
        )));
    }
    Ok(Some(bytes))
}

/// The packet embedded in the photo file at `path`, at most
/// [`MAX_XMP_BYTES`]; `None` when it has none or is not a JPEG or TIFF-based
/// file.
pub fn embedded_packet(path: &Path) -> Result<Option<Vec<u8>>, SidecarError> {
    embedded::embedded_packet(path, MAX_XMP_BYTES).map_err(SidecarError::from)
}

fn in_file(location: &str, error: SidecarError) -> SidecarError {
    SidecarError::Invalid(format!("{location}: {error}"))
}
