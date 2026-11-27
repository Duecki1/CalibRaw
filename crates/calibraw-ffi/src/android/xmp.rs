//! The XMP of library photos, reached through Android storage, so their
//! ratings follow the XMP settings as desktop photos do
//! ([`calibraw_core::sidecar::xmp_review`]).

use super::*;
use calibraw_core::display_metadata_cache::XmpSidecarStamps;
use calibraw_core::sidecar::xmp_review::{self, XmpFiles, XmpSidecarName};
use calibraw_core::sidecar::{PhotoReview, SidecarError};
use std::sync::{Mutex, PoisonError};

/// Held while an XMP sidecar is read, changed and written back. Photos that
/// differ only in extension share a basename sidecar ("IMG_0001.xmp"), and
/// the rating saver and the open photo's edit save may change two of them at
/// once. A leaf lock: nothing else is taken while it is held, and Java does
/// not call back into native code from the storage calls made under it.
static XMP_SAVES: Mutex<()> = Mutex::new(());

/// A library photo's XMP: the sidecars `StorageManager` keeps beside it in
/// CalibRaw's library or a linked folder, and the packet embedded in it.
struct AndroidXmpFiles<'a> {
    app: &'a AndroidApp,
    raw_uri: &'a str,
    display_name: &'a str,
    /// A readable path to the photo while the caller has it open, so the
    /// embedded packet is read without opening the photo again.
    photo: Option<&'a Path>,
    /// The XMP sidecars a library listing showed, taken since this process
    /// last wrote the photo's; a name it shows absent is not asked for. In a
    /// linked folder that would list the whole folder again.
    listed: Option<XmpSidecarStamps>,
}

impl XmpFiles for AndroidXmpFiles<'_> {
    fn sidecar_location(&self, name: XmpSidecarName) -> String {
        // The names `AndroidStorageContract.xmpDisplayName` gives.
        match name {
            XmpSidecarName::WithExtension => format!("{}.xmp", self.display_name),
            XmpSidecarName::Basename => Path::new(self.display_name)
                .with_extension("xmp")
                .display()
                .to_string(),
        }
    }

    fn photo_location(&self) -> String {
        self.display_name.to_owned()
    }

    fn read_sidecar(&self, name: XmpSidecarName) -> Result<Option<Vec<u8>>, SidecarError> {
        if let Some(listed) = self.listed {
            let stamp = match name {
                XmpSidecarName::WithExtension => listed.with_extension,
                XmpSidecarName::Basename => listed.basename,
            };
            if stamp.is_none() {
                return Ok(None);
            }
        }
        let staged = with_storage_manager(self.app, |env, storage_manager| {
            let raw_uri = env.new_string(self.raw_uri)?;
            let display_name = env.new_string(self.display_name)?;
            let value = env.call_method(
                storage_manager,
                jni::jni_str!("materializeXmpSidecar"),
                jni::jni_sig!((JString, JString, bool) -> JString),
                &[
                    JValue::Object(&raw_uri),
                    JValue::Object(&display_name),
                    JValue::Bool(is_basename(name)),
                ],
            )?;
            java_string(env, value)
        })
        .map_err(|error| SidecarError::Platform(format!("could not copy it: {error:#}")))?;
        if staged.is_empty() {
            return Ok(None);
        }
        let staged = PathBuf::from(staged);
        let bytes = xmp_review::read_xmp_file(&staged);
        remove_staged(&staged);
        bytes
    }

    fn write_sidecar(&self, name: XmpSidecarName, bytes: &[u8]) -> Result<(), SidecarError> {
        let staged = with_storage_manager(self.app, |env, storage_manager| {
            let value = env.call_method(
                storage_manager,
                jni::jni_str!("createXmpSidecarCache"),
                jni::jni_sig!(() -> JString),
                &[],
            )?;
            java_string(env, value)
        })
        .map_err(|error| {
            SidecarError::Platform(format!("could not create a staging file: {error:#}"))
        })?;
        let staged = non_empty_path(staged, "Android returned no XMP staging file")
            .map_err(SidecarError::Platform)?;
        let result = calibraw_core::sidecar::write_synced(&staged, bytes)
            .and_then(|()| self.publish(&staged, name));
        remove_staged(&staged);
        result
    }

    fn embedded_packet(&self) -> Result<Option<Vec<u8>>, SidecarError> {
        match self.photo {
            Some(photo) => xmp_review::embedded_packet(photo),
            None => read_library_photo(self.app, self.raw_uri, self.display_name, "XMP", |photo| {
                xmp_review::embedded_packet(photo).map_err(|error| error.to_string())
            })
            .map_err(SidecarError::Platform),
        }
    }
}

impl AndroidXmpFiles<'_> {
    /// Stores the file written at `staged` as the sidecar called `name`.
    fn publish(&self, staged: &Path, name: XmpSidecarName) -> Result<(), SidecarError> {
        let staged = staged.to_str().ok_or_else(|| {
            SidecarError::Platform("the XMP staging path is not valid UTF-8".to_owned())
        })?;
        let published = with_storage_manager(self.app, |env, storage_manager| {
            let staged = env.new_string(staged)?;
            let raw_uri = env.new_string(self.raw_uri)?;
            let display_name = env.new_string(self.display_name)?;
            env.call_method(
                storage_manager,
                jni::jni_str!("publishXmpSidecar"),
                jni::jni_sig!((JString, JString, JString, bool) -> void),
                &[
                    JValue::Object(&staged),
                    JValue::Object(&raw_uri),
                    JValue::Object(&display_name),
                    JValue::Bool(is_basename(name)),
                ],
            )?;
            Ok(())
        })
        .map_err(|error| SidecarError::Platform(format!("could not save it: {error:#}")));
        // Even a failed publish may have replaced the sidecar, which a library
        // listing taken before does not show.
        note_sidecar_written(self.raw_uri);
        published
    }
}

/// How `StorageManager` is told which name to use.
const fn is_basename(name: XmpSidecarName) -> bool {
    matches!(name, XmpSidecarName::Basename)
}

fn remove_staged(path: &Path) {
    if let Err(error) = fs::remove_file(path) {
        log::warn!(
            "could not remove XMP staging file {}: {error}",
            path.display()
        );
    }
}

/// The review of a library photo, from the review in its CalibRaw sidecar
/// (`None` when it has none) and, as the XMP settings say, its XMP. `photo`
/// is a readable path to the photo while the caller has it open.
pub fn resolve_android_review(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
    sidecar_review: Option<PhotoReview>,
    photo: Option<&Path>,
) -> PhotoReview {
    let files = AndroidXmpFiles {
        app,
        raw_uri,
        display_name,
        photo,
        listed: None,
    };
    xmp_review::resolve_review(&files, sidecar_review)
}

/// The review a library photo's XMP records, its `preferred` sidecar name
/// first ([`xmp_review::read_xmp_review`]). `listed` is what a library
/// listing taken since this process last wrote the photo's XMP shows of its
/// sidecars; only those it shows are read.
pub fn read_android_xmp_review(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
    preferred: XmpSidecarName,
    listed: Option<XmpSidecarStamps>,
) -> xmp_review::XmpReview {
    let files = AndroidXmpFiles {
        app,
        raw_uri,
        display_name,
        photo: None,
        listed,
    };
    xmp_review::read_xmp_review(&files, preferred)
}

/// Saves `review` to a library photo's XMP sidecar when the XMP settings
/// write XMP; a change of review is saved here before its CalibRaw sidecar,
/// as on desktop.
pub fn save_android_xmp_review(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
    review: PhotoReview,
) -> Result<(), SidecarError> {
    let files = AndroidXmpFiles {
        app,
        raw_uri,
        display_name,
        photo: None,
        listed: None,
    };
    let _saving = XMP_SAVES.lock().unwrap_or_else(PoisonError::into_inner);
    xmp_review::save_review(&files, review)
}
