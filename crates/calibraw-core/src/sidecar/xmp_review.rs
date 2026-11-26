//! Ratings and flags in XMP, the metadata other photo apps read and write,
//! kept alongside the review in a photo's CalibRaw sidecar.
//!
//! [`XmpReviewSync`] decides how the two relate. CalibRaw's sidecar keeps the
//! review in every mode, so turning XMP off loses nothing. The settings are
//! process-wide ([`set_xmp_review_settings`]), as every path that reads or
//! saves a desktop sidecar must apply them alike: the library, Develop,
//! pasting and resetting edits, and the rating saver.
//!
//! Only sidecars of photos in folders (desktop paths) are involved; the
//! Android media library keeps its reviews in CalibRaw sidecars alone.

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
/// masks reach a few megabytes.
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

pub(super) fn xmp_review_settings() -> XmpReviewSettings {
    *SETTINGS.read().unwrap_or_else(PoisonError::into_inner)
}

/// The review of the photo at `raw_path`, from the review in its CalibRaw
/// sidecar (`None` when it has no sidecar) and, as the settings say, its XMP.
pub fn resolve_photo_review(raw_path: &Path, sidecar_review: Option<PhotoReview>) -> PhotoReview {
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
    read_xmp_review(raw_path, settings.sidecar_name).over(base)
}

/// The review the photo's XMP records: from the first of its XMP sidecars
/// that records one, `preferred` name first, else from the packet embedded in
/// the photo file. An unreadable file is logged and passed over.
fn read_xmp_review(raw_path: &Path, preferred: XmpSidecarName) -> XmpReview {
    for path in sidecar_paths(raw_path, preferred) {
        match read_xmp_file(&path)
            .and_then(|bytes| bytes.map(|bytes| mapping::read_review(&bytes)).transpose())
        {
            Ok(Some(review)) if !review.is_empty() => return review,
            Ok(_) => {}
            Err(error) => log::warn!("could not read the rating in {}: {error}", path.display()),
        }
    }
    match embedded::embedded_packet(raw_path, MAX_XMP_BYTES) {
        Ok(Some(bytes)) => mapping::read_review(&bytes).unwrap_or_else(|error| {
            log::warn!(
                "could not read the rating in {}: {error}",
                raw_path.display()
            );
            XmpReview::default()
        }),
        Ok(None) => XmpReview::default(),
        Err(error) => {
            log::warn!("could not read the XMP in {}: {error}", raw_path.display());
            XmpReview::default()
        }
    }
}

/// Saves `review` to the photo's XMP sidecar: the first that records a
/// review, `preferred` name first, else the first that exists, else a new
/// sidecar with the preferred name. Every other byte of an existing sidecar
/// is kept; one CalibRaw cannot read is left alone and reported.
pub(super) fn save_xmp_review(
    raw_path: &Path,
    preferred: XmpSidecarName,
    review: PhotoReview,
) -> Result<(), SidecarError> {
    let mut existing = Vec::new();
    for path in sidecar_paths(raw_path, preferred) {
        if let Some(bytes) = read_xmp_file(&path).map_err(|error| in_file(&path, error))? {
            existing.push((path, bytes));
        }
    }
    let records_review =
        |bytes: &[u8]| mapping::read_review(bytes).is_ok_and(|found| !found.is_empty());
    let target = existing
        .iter()
        .position(|(_, bytes)| records_review(bytes))
        .or((!existing.is_empty()).then_some(0));
    let (path, bytes) = match target.map(|index| &existing[index]) {
        Some((path, bytes)) => (
            path.clone(),
            mapping::write_review(bytes, review).map_err(|error| in_file(path, error))?,
        ),
        None => (
            preferred.path(raw_path),
            Some(mapping::new_sidecar(review).into_bytes()),
        ),
    };
    match bytes {
        Some(bytes) => atomic_write(&path, &bytes).map_err(|error| in_file(&path, error)),
        None => Ok(()),
    }
}

/// The photo's XMP sidecar names, `preferred` first.
fn sidecar_paths(raw_path: &Path, preferred: XmpSidecarName) -> [PathBuf; 2] {
    [preferred.path(raw_path), preferred.other().path(raw_path)]
}

/// The file at `path`; `None` when there is none.
fn read_xmp_file(path: &Path) -> Result<Option<Vec<u8>>, SidecarError> {
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

fn in_file(path: &Path, error: SidecarError) -> SidecarError {
    SidecarError::Invalid(format!("{}: {error}", path.display()))
}
