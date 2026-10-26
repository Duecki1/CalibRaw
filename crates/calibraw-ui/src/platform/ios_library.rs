//! Local iOS photo storage, independent of main-thread picker integration.

use super::service::PickedDocument;
use super::{ImportedLibraryDocument, LibraryDocument, LibraryFolder};
use calibraw_core::pipeline::{
    is_supported_image_path, load_raw_display_dimensions, load_raw_thumbnail, RawThumbnail,
};
use calibraw_core::sidecar::{self, EditState, LoadedSidecar, PhotoReview, SidecarError};
use calibraw_core::thumbnail_cache;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::UNIX_EPOCH;

/// The coordinator supplies HOME/Documents/CalibRaw and a dedicated cache directory.
/// Managed paths reject symlinks. The sandbox must prevent other processes from
/// replacing path components concurrently; this mutex serializes this owner's writes.
#[cfg_attr(
    test,
    allow(dead_code, reason = "only ios.rs calls it outside its unit tests")
)]
pub(super) struct IosLibrary {
    root: PathBuf,
    cache: PathBuf,
    folder: Mutex<String>,
}

struct PartialCopy {
    path: PathBuf,
    committed: bool,
}

pub(super) struct IosImportStaging {
    directory: PathBuf,
}

impl IosImportStaging {
    pub(super) fn new(directory: PathBuf, temporary_root: &Path) -> Result<Self, String> {
        let metadata = fs::symlink_metadata(&directory).map_err(|error| error.to_string())?;
        let root = fs::canonicalize(temporary_root).map_err(|error| error.to_string())?;
        let parent = directory
            .parent()
            .ok_or_else(|| "Invalid import staging directory".to_owned())?;
        let parent = fs::canonicalize(parent).map_err(|error| error.to_string())?;
        let name = directory
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || parent != root
            || !name.starts_with("CalibRaw-import-")
        {
            return Err("Import staging must be an owned temporary directory".to_owned());
        }
        Ok(Self { directory })
    }
}

impl Drop for IosImportStaging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

impl Drop for PartialCopy {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg_attr(
    test,
    allow(dead_code, reason = "only ios.rs calls it outside its unit tests")
)]
impl IosLibrary {
    pub(super) fn new(root: PathBuf, cache: PathBuf) -> Result<Self, String> {
        let root = prepare_directory(&root)?;
        let cache = prepare_directory(&cache)?;
        if cache.starts_with(&root) || root.starts_with(&cache) {
            return Err("library and thumbnail cache must be separate directories".to_owned());
        }
        Ok(Self {
            root,
            cache,
            folder: Mutex::new(String::new()),
        })
    }

    fn state(&self) -> Result<MutexGuard<'_, String>, String> {
        self.folder
            .lock()
            .map_err(|_| "iOS library storage lock is poisoned".to_owned())
    }

    fn resolve(&self, uri: &str) -> Result<PathBuf, String> {
        validate_relative(uri, false)?;
        checked_join(&self.root, uri)
    }

    fn photo(&self, uri: &str) -> Result<PathBuf, String> {
        let path = self.resolve(uri)?;
        if !path.is_file() || !is_supported_image_path(&path) {
            return Err("library document is not a supported photo".to_owned());
        }
        self.check_companions(&path)?;
        Ok(path)
    }

    fn check_companions(&self, raw: &Path) -> Result<(), String> {
        for path in sidecar::raw_companion_paths(raw) {
            let uri = path
                .strip_prefix(&self.root)
                .map_err(|error| error.to_string())?
                .to_str()
                .ok_or_else(|| "non-UTF-8 library path".to_owned())?;
            self.resolve(uri)?;
        }
        Ok(())
    }

    fn folder_path(&self, relative: &str) -> Result<PathBuf, String> {
        validate_relative(relative, true)?;
        let path = checked_join(&self.root, relative)?;
        if !path.is_dir() {
            return Err("library folder does not exist".to_owned());
        }
        Ok(path)
    }

    pub(super) fn library_location(&self) -> Result<String, String> {
        let folder = self.state()?;
        self.folder_path(&folder)?;
        Ok(if folder.is_empty() {
            "Documents/CalibRaw".to_owned()
        } else {
            format!("Documents/CalibRaw/{folder}")
        })
    }

    pub(super) fn list_library_documents(&self) -> Result<Vec<LibraryDocument>, String> {
        let folder = self.state()?;
        let mut documents = Vec::new();
        for entry in fs::read_dir(self.folder_path(&folder)?).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            if !entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_file()
                || !is_supported_image_path(&entry.path())
            {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if validate_name(&name).is_err() {
                continue;
            }
            let uri = relative_child(&folder, &name);
            self.photo(&uri)?;
            let metadata = entry.metadata().map_err(|error| error.to_string())?;
            documents.push(LibraryDocument {
                display_path: uri.clone(),
                uri,
                display_name: name,
                bytes: metadata.len(),
                modified_seconds: metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |time| time.as_secs()),
            });
        }
        documents.sort_by(|left, right| left.uri.cmp(&right.uri));
        Ok(documents)
    }

    pub(super) fn list_library_folders(&self) -> Result<Vec<LibraryFolder>, String> {
        let _state = self.state()?;
        let mut folders = Vec::new();
        let mut pending = vec![String::new()];
        while let Some(parent) = pending.pop() {
            for entry in
                fs::read_dir(self.folder_path(&parent)?).map_err(|error| error.to_string())?
            {
                let entry = entry.map_err(|error| error.to_string())?;
                if !entry
                    .file_type()
                    .map_err(|error| error.to_string())?
                    .is_dir()
                {
                    continue;
                }
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if validate_name(&name).is_err() {
                    continue;
                }
                let path = relative_child(&parent, &name);
                self.folder_path(&path)?;
                pending.push(path.clone());
                folders.push(LibraryFolder { path, name });
            }
        }
        folders.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(folders)
    }

    pub(super) fn select_library_folder(&self, relative_path: &str) -> Result<(), String> {
        let mut folder = self.state()?;
        self.folder_path(relative_path)?;
        *folder = relative_path.to_owned();
        Ok(())
    }

    pub(super) fn create_library_folder(&self, parent: &str, name: &str) -> Result<String, String> {
        let _state = self.state()?;
        validate_name(name)?;
        self.folder_path(parent)?;
        let uri = relative_child(parent, name);
        fs::create_dir(self.resolve(&uri)?).map_err(|error| error.to_string())?;
        Ok(uri)
    }

    pub(super) fn materialize_library_document(
        &self,
        uri: &str,
        _display_name: &str,
    ) -> Result<PathBuf, String> {
        let _state = self.state()?;
        self.photo(uri)
    }

    pub(super) fn open_library_document_data(
        &self,
        uri: &str,
        _display_name: &str,
    ) -> Result<PickedDocument, String> {
        let _state = self.state()?;
        let path = self.photo(uri)?;
        let display_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "invalid photo name".to_owned())?
            .to_owned();
        Ok(PickedDocument {
            path,
            display_name,
            library_uri: uri.to_owned(),
            delete_after_decode: false,
            raw_fd_guard: None,
        })
    }

    pub(super) fn import_local_library_document(
        &self,
        raw_path: &Path,
        display_name: &str,
    ) -> Result<ImportedLibraryDocument, String> {
        let folder = self.state()?;
        validate_name(display_name)?;
        if !is_supported_image_path(Path::new(display_name)) {
            return Err("unsupported photo type".to_owned());
        }
        self.folder_path(&folder)?;
        check_path_component(raw_path)?;
        let mut source = File::open(raw_path).map_err(|error| error.to_string())?;
        if !source
            .metadata()
            .map_err(|error| error.to_string())?
            .is_file()
        {
            return Err("import source is not a regular file".to_owned());
        }
        for index in 0_u64.. {
            let name = collision_name(display_name, index)?;
            let uri = relative_child(&folder, &name);
            let path = self.resolve(&uri)?;
            self.check_companions(&path)?;
            if sidecar::raw_companion_paths(&path)
                .iter()
                .any(|path| path.exists())
            {
                continue;
            }
            let mut destination = match OpenOptions::new().write(true).create_new(true).open(&path)
            {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.to_string()),
            };
            let mut partial = PartialCopy {
                path,
                committed: false,
            };
            io::copy(&mut source, &mut destination).map_err(|error| error.to_string())?;
            destination.sync_all().map_err(|error| error.to_string())?;
            partial.committed = true;
            return Ok(ImportedLibraryDocument {
                uri,
                display_name: name,
            });
        }
        Err("no available photo name".to_owned())
    }

    pub(super) fn delete_imported_library_document(
        &self,
        uri: &str,
        display_name: &str,
    ) -> Result<(), String> {
        self.delete_library_document(uri, display_name)
    }

    pub(super) fn rename_library_document(
        &self,
        uri: &str,
        _display_name: &str,
        requested_name: &str,
    ) -> Result<String, String> {
        let _state = self.state()?;
        validate_name(requested_name)?;
        let source = self.photo(uri)?;
        let extension = source
            .extension()
            .and_then(|value| value.to_str())
            .ok_or_else(|| "photo has no extension".to_owned())?;
        let name = if Path::new(requested_name).extension().is_none() {
            format!("{requested_name}.{extension}")
        } else {
            requested_name.to_owned()
        };
        validate_name(&name)?;
        if Path::new(&name)
            .extension()
            .and_then(|value| value.to_str())
            .is_none_or(|value| !value.eq_ignore_ascii_case(extension))
        {
            return Err("renaming cannot change the photo format".to_owned());
        }
        let parent = uri.rsplit_once('/').map_or("", |(parent, _)| parent);
        let destination_uri = relative_child(parent, &name);
        let destination = self.resolve(&destination_uri)?;
        if source == destination {
            return Ok(name);
        }
        self.check_companions(&destination)?;
        let sources = [
            source.clone(),
            sidecar::sidecar_path_for_raw(&source),
            sidecar::ai_denoise_path_for_raw(&source),
        ];
        let destinations = [
            destination.clone(),
            sidecar::sidecar_path_for_raw(&destination),
            sidecar::ai_denoise_path_for_raw(&destination),
        ];
        if destinations.iter().any(|path| path.exists()) {
            return Err("a photo or its companion already uses that name".to_owned());
        }
        self.invalidate_cache(uri)?;
        self.invalidate_cache(&destination_uri)?;
        let mut linked = Vec::new();
        for (source_path, destination_path) in sources.iter().zip(&destinations) {
            if !source_path.exists() {
                continue;
            }
            if let Err(error) = fs::hard_link(source_path, destination_path) {
                for (_, path) in linked {
                    let _ = fs::remove_file(path);
                }
                return Err(error.to_string());
            }
            linked.push((source_path, destination_path));
        }
        let mut removed = Vec::new();
        for (source_path, destination_path) in &linked {
            if let Err(error) = fs::remove_file(source_path) {
                for (old_path, new_path) in removed {
                    if let Err(restore_error) = fs::hard_link(new_path, old_path) {
                        return Err(format!(
                            "rename failed: {error}; restoring original name failed: {restore_error}; new names retained to preserve data"
                        ));
                    }
                }
                for (_, new_path) in &linked {
                    let _ = fs::remove_file(new_path);
                }
                return Err(error.to_string());
            }
            removed.push((source_path, destination_path));
        }
        Ok(name)
    }

    pub(super) fn delete_library_document(
        &self,
        uri: &str,
        _display_name: &str,
    ) -> Result<(), String> {
        let _state = self.state()?;
        let raw = self.photo(uri)?;
        self.invalidate_cache(uri)?;
        for path in sidecar::raw_companion_paths(&raw) {
            remove_optional(&path)?;
        }
        fs::remove_file(raw).map_err(|error| error.to_string())
    }

    pub(super) fn load_sidecar(
        &self,
        uri: &str,
        _display_name: &str,
    ) -> Result<Option<LoadedSidecar>, SidecarError> {
        let _state = self.state().map_err(sidecar_error)?;
        sidecar::load_desktop(&self.photo(uri).map_err(sidecar_error)?)
    }

    pub(super) fn load_sidecar_review(
        &self,
        uri: &str,
        _display_name: &str,
    ) -> Result<Option<PhotoReview>, SidecarError> {
        let _state = self.state().map_err(sidecar_error)?;
        let raw = self.photo(uri).map_err(sidecar_error)?;
        if !sidecar::sidecar_path_for_raw(&raw).exists() {
            return Ok(None);
        }
        sidecar::load_photo_review(&raw).map(Some)
    }

    pub(super) fn save_sidecar(
        &self,
        uri: &str,
        _display_name: &str,
        edits: EditState,
    ) -> Result<String, SidecarError> {
        let _state = self.state().map_err(sidecar_error)?;
        sidecar::save_desktop(&self.photo(uri).map_err(sidecar_error)?, edits)?;
        self.invalidate_developed(uri).map_err(sidecar_error)?;
        Ok(format!("{uri}{}", sidecar::SIDECAR_SUFFIX))
    }

    pub(super) fn save_sidecar_with_review_and_editing_time(
        &self,
        uri: &str,
        _display_name: &str,
        edits: EditState,
        review: PhotoReview,
        editing_time_ms: u64,
    ) -> Result<String, SidecarError> {
        let _state = self.state().map_err(sidecar_error)?;
        let raw = self.photo(uri).map_err(sidecar_error)?;
        if review.rating > 5 {
            return Err(SidecarError::Invalid(
                "rating must be between 0 and 5".to_owned(),
            ));
        }
        sidecar::load_sidecar_metadata(&raw)?;
        let uses_ai_denoise = edits.exposure.ai_denoise_enabled;
        let bytes = sidecar::encode_with_review_and_editing_time(edits, review, editing_time_ms)?;
        calibraw_core::file_ops::write_atomically(&sidecar::sidecar_path_for_raw(&raw), |file| {
            file.write_all(&bytes).map_err(SidecarError::Io)
        })?;
        if !uses_ai_denoise {
            remove_optional(&sidecar::ai_denoise_path_for_raw(&raw)).map_err(sidecar_error)?;
        }
        self.invalidate_developed(uri).map_err(sidecar_error)?;
        Ok(format!("{uri}{}", sidecar::SIDECAR_SUFFIX))
    }

    pub(super) fn reset_adjustments(
        &self,
        uri: &str,
        _display_name: &str,
        editing_time_ms: Option<u64>,
    ) -> Result<(), String> {
        let _state = self.state()?;
        let raw = self.photo(uri)?;
        let metadata = sidecar::load_sidecar_metadata(&raw).map_err(|error| error.to_string())?;
        let time = editing_time_ms.unwrap_or(metadata.editing_time_ms);
        if metadata.review == PhotoReview::default() && time == 0 {
            remove_optional(&sidecar::sidecar_path_for_raw(&raw))?;
        } else {
            sidecar::save_desktop_with_editing_time(&raw, sidecar::default_edit_state(), time)
                .map_err(|error| error.to_string())?;
        }
        remove_optional(&sidecar::ai_denoise_path_for_raw(&raw))?;
        self.invalidate_developed(uri)
    }

    pub(super) fn materialize_raw_sidecar(
        &self,
        uri: &str,
        _display_name: &str,
    ) -> Result<Option<PathBuf>, String> {
        let _state = self.state()?;
        let path = sidecar::sidecar_path_for_raw(&self.photo(uri)?);
        Ok(path.is_file().then_some(path))
    }

    pub(super) fn ai_denoise_result_path(
        &self,
        uri: &str,
        _display_name: &str,
    ) -> Result<PathBuf, String> {
        let _state = self.state()?;
        Ok(sidecar::ai_denoise_path_for_raw(&self.photo(uri)?))
    }

    fn cache_path(&self, uri: &str, suffix: &str) -> Result<PathBuf, String> {
        validate_relative(uri, false)?;
        checked_join(
            &self.cache,
            &format!("{:016x}{suffix}", thumbnail_cache::fnv1a64(uri.as_bytes())),
        )
    }

    fn invalidate_developed(&self, uri: &str) -> Result<(), String> {
        for suffix in [".calibraw-thumb.jpg", ".calibraw-thumb.fingerprint"] {
            remove_optional(&self.cache_path(uri, suffix)?)?;
        }
        Ok(())
    }

    fn invalidate_cache(&self, uri: &str) -> Result<(), String> {
        self.invalidate_developed(uri)?;
        for suffix in [".raw.jpg", ".raw.fingerprint"] {
            remove_optional(&self.cache_path(uri, suffix)?)?;
        }
        Ok(())
    }

    pub(super) fn load_library_thumbnail(
        &self,
        uri: &str,
        _display_name: &str,
        _bytes: u64,
        _modified_seconds: u64,
        maximum_edge: u32,
    ) -> Result<RawThumbnail, String> {
        let _state = self.state()?;
        if maximum_edge == 0 {
            return Err("thumbnail edge must be non-zero".to_owned());
        }
        let raw = self.photo(uri)?;
        let stamp = format!("{}:{maximum_edge}", raw_stamp(&raw)?);
        let path = self.cache_path(uri, ".raw.jpg")?;
        let fingerprint = self.cache_path(uri, ".raw.fingerprint")?;
        if read_optional_text(&fingerprint)?.as_deref() == Some(&stamp) {
            if let Some(thumbnail) = thumbnail_cache::load_jpeg(&path, maximum_edge)? {
                return Ok(thumbnail);
            }
        }
        let thumbnail =
            load_raw_thumbnail(&raw, maximum_edge).map_err(|error| error.to_string())?;
        thumbnail_cache::save_jpeg(&path, &thumbnail)?;
        calibraw_core::file_ops::write_bytes_atomically(&fingerprint, stamp.as_bytes())
            .map_err(|error| error.to_string())?;
        Ok(thumbnail)
    }

    pub(super) fn load_library_display_dimensions(&self, uri: &str) -> Result<[u32; 2], String> {
        let _state = self.state()?;
        load_raw_display_dimensions(&self.photo(uri)?).map_err(|error| error.to_string())
    }

    fn load_developed(&self, uri: &str, maximum_edge: u32) -> Result<Option<RawThumbnail>, String> {
        if maximum_edge == 0 {
            return Err("thumbnail edge must be non-zero".to_owned());
        }
        let raw = self.photo(uri)?;
        let Some(fingerprint) = sidecar::desktop_sidecar_fingerprint(&raw)? else {
            return Ok(None);
        };
        let expected = format!(
            "{:016x}\n",
            fingerprint ^ sidecar::DEVELOPED_THUMBNAIL_CACHE_VERSION_SALT
        );
        if read_optional_text(&self.cache_path(uri, ".calibraw-thumb.fingerprint")?)?.as_deref()
            != Some(&expected)
        {
            return Ok(None);
        }
        let path = self.cache_path(uri, ".calibraw-thumb.jpg")?;
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        if metadata.modified().map_err(|error| error.to_string())?
            < fs::metadata(raw)
                .and_then(|metadata| metadata.modified())
                .map_err(|error| error.to_string())?
        {
            return Ok(None);
        }
        thumbnail_cache::load_jpeg(&path, maximum_edge)
    }

    pub(super) fn load_developed_thumbnail_cache(
        &self,
        uri: &str,
        _display_name: &str,
        maximum_edge: u32,
    ) -> Result<Option<RawThumbnail>, String> {
        let _state = self.state()?;
        self.load_developed(uri, maximum_edge)
    }

    fn save_developed(&self, uri: &str, thumbnail: &RawThumbnail) -> Result<(), String> {
        let raw = self.photo(uri)?;
        let fingerprint = sidecar::desktop_sidecar_fingerprint(&raw)?
            .ok_or_else(|| "developed thumbnail requires a saved sidecar".to_owned())?;
        thumbnail_cache::save_jpeg(&self.cache_path(uri, ".calibraw-thumb.jpg")?, thumbnail)?;
        calibraw_core::file_ops::write_bytes_atomically(
            &self.cache_path(uri, ".calibraw-thumb.fingerprint")?,
            format!(
                "{:016x}\n",
                fingerprint ^ sidecar::DEVELOPED_THUMBNAIL_CACHE_VERSION_SALT
            )
            .as_bytes(),
        )
        .map_err(|error| error.to_string())
    }

    pub(super) fn save_developed_thumbnail_cache(
        &self,
        uri: &str,
        _display_name: &str,
        thumbnail: &RawThumbnail,
    ) -> Result<(), String> {
        let _state = self.state()?;
        self.save_developed(uri, thumbnail)
    }

    pub(super) fn copy_library_developed_thumbnail_cache(
        &self,
        source_uri: &str,
        destination_uri: &str,
    ) -> Result<(), String> {
        let _state = self.state()?;
        self.photo(destination_uri)?;
        if let Some(thumbnail) = self.load_developed(source_uri, 8192)? {
            self.save_developed(destination_uri, &thumbnail)?;
        }
        Ok(())
    }

    pub(super) fn clear_thumbnail_cache(&self) -> Result<(), String> {
        let _state = self.state()?;
        for entry in
            fs::read_dir(checked_join(&self.cache, "")?).map_err(|error| error.to_string())?
        {
            let entry = entry.map_err(|error| error.to_string())?;
            if !entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_file()
            {
                return Err("unexpected non-file in thumbnail cache".to_owned());
            }
            fs::remove_file(entry.path()).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub(super) fn thumbnail_cache_size_bytes(&self) -> Result<u64, String> {
        let _state = self.state()?;
        let mut size = 0_u64;
        for entry in
            fs::read_dir(checked_join(&self.cache, "")?).map_err(|error| error.to_string())?
        {
            let entry = entry.map_err(|error| error.to_string())?;
            if !entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_file()
            {
                return Err("unexpected non-file in thumbnail cache".to_owned());
            }
            size = size.saturating_add(entry.metadata().map_err(|error| error.to_string())?.len());
        }
        Ok(size)
    }
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.trim() != name
        || name.len() > 255
        || name.contains(['/', '\\', ':'])
        || name.chars().any(char::is_control)
    {
        return Err("invalid library name".to_owned());
    }
    Ok(())
}

fn validate_relative(relative: &str, allow_root: bool) -> Result<(), String> {
    if relative.is_empty() && allow_root {
        return Ok(());
    }
    for component in relative.split('/') {
        validate_name(component)?;
    }
    Ok(())
}

fn checked_join(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let mut path = root.to_path_buf();
    check_path_component(&path)?;
    if fs::canonicalize(root).map_err(|error| error.to_string())? != root {
        return Err("managed storage root or its ancestors changed".to_owned());
    }
    for component in relative
        .split('/')
        .filter(|component| !component.is_empty())
    {
        path.push(component);
        check_path_component(&path)?;
    }
    Ok(path)
}

fn check_path_component(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err("symlinks are not managed library paths".to_owned())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn prepare_directory(path: &Path) -> Result<PathBuf, String> {
    check_path_component(path)?;
    fs::create_dir_all(path).map_err(|error| error.to_string())?;
    fs::canonicalize(path).map_err(|error| error.to_string())
}

fn relative_child(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

fn collision_name(name: &str, index: u64) -> Result<String, String> {
    if index == 0 {
        return Ok(name.to_owned());
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "invalid photo name".to_owned())?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "photo has no extension".to_owned())?;
    let result = format!("{stem}-{index}.{extension}");
    validate_name(&result)?;
    Ok(result)
}

fn remove_optional(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn read_optional_text(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn raw_stamp(raw: &Path) -> Result<String, String> {
    let metadata = fs::metadata(raw).map_err(|error| error.to_string())?;
    let modified = metadata
        .modified()
        .map_err(|error| error.to_string())?
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?;
    Ok(format!("{}:{}", metadata.len(), modified.as_nanos()))
}

fn sidecar_error(message: String) -> SidecarError {
    SidecarError::Io(io::Error::other(message))
}
