//! The Android photo library: folders, documents, import, rename and delete.

use super::*;

pub fn library_location(app: &AndroidApp) -> Result<String, String> {
    with_storage_manager(app, |env, storage_manager| {
        let value = env.call_method(
            storage_manager,
            jni::jni_str!("rawLibraryLocation"),
            jni::jni_sig!(() -> JString),
            &[],
        )?;
        java_string(env, value)
    })
    .map_err(|error| format!("could not locate Android RAW library: {error:#}"))
}

pub fn list_library_documents(app: &AndroidApp) -> Result<Vec<LibraryDocument>, String> {
    let encoded = with_storage_manager(app, |env, storage_manager| {
        let value = env.call_method(
            storage_manager,
            jni::jni_str!("listRawLibrary"),
            jni::jni_sig!(() -> JString),
            &[],
        )?;
        java_string(env, value)
    })
    .map_err(|error| format!("could not list Android RAW library: {error:#}"))?;
    encoded
        .lines()
        .filter(|line| !line.is_empty())
        .map(parse_library_record)
        .collect()
}

/// One `listRawLibrary` line: the encoded URI, name and path, the byte size,
/// the modification time in seconds, then the sidecar's byte size and
/// modification time in milliseconds, both `-` when the photo has none.
fn parse_library_record(line: &str) -> Result<LibraryDocument, String> {
    fn number(field: Option<&str>, what: &str) -> Result<u64, String> {
        field
            .ok_or_else(|| format!("Android library record has no {what}"))?
            .parse::<u64>()
            .map_err(|error| format!("invalid Android library {what}: {error}"))
    }
    let mut fields = line.split('\t');
    let uri = decode_uri_component(fields.next().unwrap_or_default())?;
    let display_name = decode_uri_component(fields.next().unwrap_or_default())?;
    let display_path = decode_uri_component(fields.next().unwrap_or_default())?;
    let bytes = number(fields.next(), "byte size")?;
    let modified_seconds = number(fields.next(), "modification time")?;
    let sidecar = match (fields.next(), fields.next()) {
        (Some("-"), Some("-")) => None,
        (sidecar_bytes, sidecar_modified_millis) => {
            Some(calibraw_core::display_metadata_cache::FileStamp::new(
                number(sidecar_bytes, "sidecar byte size")?,
                number(sidecar_modified_millis, "sidecar modification time")?
                    .saturating_mul(1_000_000),
            ))
        }
    };
    if fields.next().is_some() || uri.is_empty() || display_name.is_empty() {
        return Err("malformed Android library record".to_owned());
    }
    Ok(LibraryDocument {
        uri,
        display_name,
        display_path,
        bytes,
        modified_seconds,
        sidecar,
    })
}

pub fn list_library_folders(app: &AndroidApp) -> Result<Vec<LibraryFolder>, String> {
    let encoded = with_storage_manager(app, |env, storage_manager| {
        let object = env
            .call_method(
                storage_manager,
                jni::jni_str!("listRawLibraryFolders"),
                jni::jni_sig!(() -> JString),
                &[],
            )?
            .l()?;
        Ok(env.cast_local::<JString>(object)?.to_string())
    })
    .map_err(|error| format!("could not list Android RAW library folders: {error:#}"))?;
    parse_library_folders(&encoded)
}

/// The folders of the linked folder `key`, whichever folder is selected.
pub fn list_linked_folder_tree(app: &AndroidApp, key: &str) -> Result<Vec<LibraryFolder>, String> {
    let encoded = with_storage_manager(app, |env, storage_manager| {
        let key = env.new_string(key)?;
        let value = env.call_method(
            storage_manager,
            jni::jni_str!("listLinkedFolderTree"),
            jni::jni_sig!((JString) -> JString),
            &[JValue::Object(&key)],
        )?;
        java_string(env, value)
    })
    .map_err(|error| format!("could not list the linked folder's folders: {error:#}"))?;
    parse_library_folders(&encoded)
}

/// `{encoded path}\t{encoded name}` lines, parents first.
fn parse_library_folders(encoded: &str) -> Result<Vec<LibraryFolder>, String> {
    encoded
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut fields = line.split('\t');
            let path = decode_uri_component(fields.next().unwrap_or_default())?;
            let name = decode_uri_component(fields.next().unwrap_or_default())?;
            if fields.next().is_some() || path.is_empty() || name.is_empty() {
                return Err("malformed Android library folder record".to_owned());
            }
            Ok(LibraryFolder { path, name })
        })
        .collect()
}

pub fn list_linked_folders(app: &AndroidApp) -> Result<Vec<LinkedFolder>, String> {
    let encoded = with_storage_manager(app, |env, storage_manager| {
        let value = env.call_method(
            storage_manager,
            jni::jni_str!("listLinkedFolders"),
            jni::jni_sig!(() -> JString),
            &[],
        )?;
        java_string(env, value)
    })
    .map_err(|error| format!("could not list linked folders: {error:#}"))?;
    encoded
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut fields = line.split('\t');
            let key = decode_uri_component(fields.next().unwrap_or_default())?;
            let label = decode_uri_component(fields.next().unwrap_or_default())?;
            if fields.next().is_some() || key.is_empty() || label.is_empty() {
                return Err("malformed linked folder record".to_owned());
            }
            Ok(LinkedFolder { key, label })
        })
        .collect()
}

pub fn unlink_folder(app: &AndroidApp, key: &str) -> Result<(), String> {
    with_storage_manager(app, |env, storage_manager| {
        let key = env.new_string(key)?;
        env.call_method(
            storage_manager,
            jni::jni_str!("unlinkFolder"),
            jni::jni_sig!((JString) -> void),
            &[JValue::Object(&key)],
        )?;
        Ok(())
    })
    .map_err(|error| format!("{error:#}"))
}

/// Lists from the top of a linked folder, or of the app's library for an
/// empty key.
pub fn select_library_root(app: &AndroidApp, key: &str) -> Result<(), String> {
    with_storage_manager(app, |env, storage_manager| {
        let key = env.new_string(key)?;
        env.call_method(
            storage_manager,
            jni::jni_str!("selectRawLibraryRoot"),
            jni::jni_sig!((JString) -> void),
            &[JValue::Object(&key)],
        )?;
        Ok(())
    })
    .map_err(|error| format!("{error:#}"))
}

pub fn select_library_folder(app: &AndroidApp, relative_path: &str) -> Result<(), String> {
    with_storage_manager(app, |env, storage_manager| {
        let relative_path = env.new_string(relative_path)?;
        env.call_method(
            storage_manager,
            jni::jni_str!("selectRawLibraryFolder"),
            jni::jni_sig!((JString) -> void),
            &[JValue::Object(&relative_path)],
        )?;
        Ok(())
    })
    .map_err(|error| format!("could not select Android RAW library folder: {error:#}"))
}

pub fn create_library_folder(
    app: &AndroidApp,
    parent_path: &str,
    name: &str,
) -> Result<String, String> {
    with_storage_manager(app, |env, storage_manager| {
        let parent_path = env.new_string(parent_path)?;
        let name = env.new_string(name)?;
        let object = env
            .call_method(
                storage_manager,
                jni::jni_str!("createRawLibraryFolder"),
                jni::jni_sig!((JString, JString) -> JString),
                &[JValue::Object(&parent_path), JValue::Object(&name)],
            )?
            .l()?;
        Ok(env.cast_local::<JString>(object)?.to_string())
    })
    .map_err(|error| format!("could not create Android RAW library folder: {error:#}"))
}

pub fn materialize_library_document(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
) -> Result<PathBuf, String> {
    let path = materialize_storage_path(
        app,
        jni::jni_str!("materializeRawLibraryDocument"),
        raw_uri,
        display_name,
    )
    .map_err(|error| format!("could not materialize Android RAW: {error:#}"))?;
    non_empty_path(path, "Android returned no RAW staging path")
}

pub(super) fn materialize_library_thumbnail(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
) -> Result<PathBuf, String> {
    let path = materialize_storage_path(
        app,
        jni::jni_str!("materializeRawLibraryThumbnail"),
        raw_uri,
        display_name,
    )
    .map_err(|error| format!("could not materialize Android RAW thumbnail: {error:#}"))?;
    non_empty_path(path, "Android returned no RAW thumbnail staging path")
}

fn materialize_storage_path(
    app: &AndroidApp,
    method: impl AsRef<jni::strings::JNIStr>,
    raw_uri: &str,
    display_name: &str,
) -> jni::errors::Result<String> {
    with_storage_manager(app, |env, storage_manager| {
        let raw_uri = env.new_string(raw_uri)?;
        let display_name = env.new_string(display_name)?;
        let object = env
            .call_method(
                storage_manager,
                method,
                jni::jni_sig!((JString, JString) -> JString),
                &[JValue::Object(&raw_uri), JValue::Object(&display_name)],
            )?
            .l()?;
        Ok(env.cast_local::<JString>(object)?.to_string())
    })
}

pub fn open_library_document(
    app: &AndroidApp,
    uri: &str,
    display_name: &str,
) -> Result<(), String> {
    with_storage_manager(app, |env, storage_manager| {
        let uri = env.new_string(uri)?;
        let display_name = env.new_string(display_name)?;
        env.call_method(
            storage_manager,
            jni::jni_str!("openRawLibraryDocument"),
            jni::jni_sig!((JString, JString) -> void),
            &[JValue::Object(&uri), JValue::Object(&display_name)],
        )?;
        Ok(())
    })
    .map_err(|error| format!("could not open Android RAW library item: {error:#}"))
}

#[derive(Debug)]
pub struct ImportedLibraryDocument {
    pub uri: String,
    pub display_name: String,
}

pub fn import_local_library_document(
    app: &AndroidApp,
    raw_path: &std::path::Path,
    display_name: &str,
) -> Result<ImportedLibraryDocument, String> {
    let raw_path = raw_path
        .to_str()
        .ok_or_else(|| "Local library staging path is not valid UTF-8".to_owned())?;
    let identity = with_storage_manager(app, |env, storage_manager| {
        let raw_path = env.new_string(raw_path)?;
        let display_name = env.new_string(display_name)?;
        let object = env
            .call_method(
                storage_manager,
                jni::jni_str!("importLocalRawLibraryDocument"),
                jni::jni_sig!((JString, JString) -> JString),
                &[JValue::Object(&raw_path), JValue::Object(&display_name)],
            )?
            .l()?;
        Ok(env.cast_local::<JString>(object)?.to_string())
    })
    .map_err(|error| format!("could not import local RAW into Android library: {error:#}"))?;
    let (uri, display_name) = identity.split_once('\n').ok_or_else(|| {
        "could not import local RAW into Android library: Android returned an invalid document identity"
            .to_owned()
    })?;
    if uri.is_empty() || display_name.is_empty() {
        return Err(
            "could not import local RAW into Android library: Android returned an empty document identity"
                .to_owned(),
        );
    }
    Ok(ImportedLibraryDocument {
        uri: uri.to_owned(),
        display_name: display_name.to_owned(),
    })
}

pub fn delete_imported_library_document(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
) -> Result<(), String> {
    with_storage_manager(app, |env, storage_manager| {
        let raw_uri = env.new_string(raw_uri)?;
        let display_name = env.new_string(display_name)?;
        env.call_method(
            storage_manager,
            jni::jni_str!("deleteImportedRawLibraryDocument"),
            jni::jni_sig!((JString, JString) -> void),
            &[JValue::Object(&raw_uri), JValue::Object(&display_name)],
        )?;
        Ok(())
    })
    .map_err(|error| format!("could not roll back imported Android RAW: {error:#}"))?;
    clear_developed_thumbnail_cache(app, raw_uri);
    Ok(())
}

pub fn rename_library_document(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
    requested_name: &str,
) -> Result<String, String> {
    let destination_uri = with_storage_manager(app, |env, storage_manager| {
        let raw_uri = env.new_string(raw_uri)?;
        let display_name = env.new_string(display_name)?;
        let requested_name = env.new_string(requested_name)?;
        let value = env.call_method(
            storage_manager,
            jni::jni_str!("renameRawLibraryDocument"),
            jni::jni_sig!((JString, JString, JString) -> JString),
            &[
                JValue::Object(&raw_uri),
                JValue::Object(&display_name),
                JValue::Object(&requested_name),
            ],
        )?;
        java_string(env, value)
    })
    .map_err(|error| format!("could not rename Android RAW library item: {error:#}"))?;
    if destination_uri != raw_uri {
        // Cached by URI, so the developed thumbnail follows the photo to its new one.
        if let Err(error) = copy_library_developed_thumbnail_cache(app, raw_uri, &destination_uri) {
            log::warn!("Renamed RAW but could not preserve its developed thumbnail: {error}");
            clear_developed_thumbnail_cache(app, &destination_uri);
        }
        clear_developed_thumbnail_cache(app, raw_uri);
    }
    Ok(destination_uri)
}

pub fn delete_library_document(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
) -> Result<(), String> {
    with_storage_manager(app, |env, storage_manager| {
        let raw_uri = env.new_string(raw_uri)?;
        let display_name = env.new_string(display_name)?;
        env.call_method(
            storage_manager,
            jni::jni_str!("deleteRawLibraryDocument"),
            jni::jni_sig!((JString, JString) -> void),
            &[JValue::Object(&raw_uri), JValue::Object(&display_name)],
        )?;
        Ok(())
    })
    .map_err(|error| format!("could not delete Android RAW library item: {error:#}"))?;
    clear_developed_thumbnail_cache(app, raw_uri);
    Ok(())
}

pub fn remove_raw_sidecar(
    app: &AndroidApp,
    raw_uri: &str,
    display_name: &str,
) -> Result<(), String> {
    let removed = with_storage_manager(app, |env, storage_manager| {
        let raw_uri = env.new_string(raw_uri)?;
        let display_name = env.new_string(display_name)?;
        env.call_method(
            storage_manager,
            jni::jni_str!("removeRawSidecar"),
            jni::jni_sig!((JString, JString) -> void),
            &[JValue::Object(&raw_uri), JValue::Object(&display_name)],
        )?;
        Ok(())
    })
    .map_err(|error| format!("could not reset Android RAW adjustments: {error:#}"));
    note_sidecar_written(raw_uri);
    removed?;
    clear_developed_thumbnail_cache(app, raw_uri);
    Ok(())
}
