//! Direct exports to picked destinations and MediaStore publishing.

use super::*;

#[derive(Debug)]
pub(super) struct DirectExportTarget {
    pub(super) descriptor: TransferredFileDescriptor,
    pub(super) uri: String,
    /// Where the export goes under its requested name. MediaStore numbers a
    /// taken name, so publishing reports the final location when it can.
    location: String,
}

#[derive(Debug)]
pub(super) struct TransferredFileDescriptor {
    _file: File,
    raw_fd: i32,
}

impl TransferredFileDescriptor {
    pub(super) fn from_java(raw_fd: i32, invalid_descriptor_message: &str) -> Result<Self, String> {
        if raw_fd < 0 {
            return Err(invalid_descriptor_message.to_owned());
        }
        let file = unsafe { File::from_raw_fd(raw_fd) };
        Ok(Self {
            _file: file,
            raw_fd,
        })
    }

    pub(super) fn proc_path(&self) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", self.raw_fd))
    }
}

#[derive(Debug)]
struct PendingExportDescriptor {
    pub(super) descriptor: TransferredFileDescriptor,
    pub(super) uri: String,
    location: String,
}

impl PendingExportDescriptor {
    pub(super) fn parse(encoded: &str) -> Result<Self, String> {
        let mut fields = encoded.splitn(3, '\t');
        let raw_fd = fields
            .next()
            .ok_or_else(|| "Android export descriptor is missing its fd".to_owned())?
            .parse::<i32>()
            .map_err(|error| format!("invalid Android export fd: {error}"))?;
        let descriptor = TransferredFileDescriptor::from_java(
            raw_fd,
            "Android export descriptor returned a negative fd",
        )?;
        let uri = fields
            .next()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "Android export descriptor is missing its URI".to_owned())?
            .to_owned();
        let location = fields
            .next()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "Android export descriptor is missing its location".to_owned())?
            .to_owned();
        Ok(Self {
            descriptor,
            uri,
            location,
        })
    }
}

pub fn prepare_direct_export(
    app: &AndroidApp,
    display_name: &str,
    mime_type: &str,
) -> Result<Option<PathBuf>, String> {
    let encoded = with_export_publisher(app, |env, export_publisher| {
        let display_name = env.new_string(display_name)?;
        let mime_type = env.new_string(mime_type)?;
        let value = env.call_method(
            export_publisher,
            jni::jni_str!("createPendingExport"),
            jni::jni_sig!((JString, JString) -> JString),
            &[JValue::Object(&display_name), JValue::Object(&mime_type)],
        )?;
        java_string(env, value)
    })
    .map_err(|error| {
        format!("could not create Android MediaStore export destination: {error:#}")
    })?;
    if encoded.is_empty() {
        return Ok(None);
    }
    let PendingExportDescriptor {
        descriptor,
        uri,
        location,
    } = PendingExportDescriptor::parse(&encoded)?;
    let path = descriptor.proc_path();
    let target = DirectExportTarget {
        descriptor,
        uri,
        location,
    };
    direct_exports()
        .lock()
        .map_err(|_| "Android direct-export state is poisoned".to_owned())?
        .insert(path.clone(), target);
    Ok(Some(path))
}

/// Publishes a finished direct export and returns where it is, under the name
/// MediaStore gave it.
pub fn finalize_direct_export(app: &AndroidApp, path: &Path) -> Result<PublishedExport, String> {
    let target = direct_exports()
        .lock()
        .map_err(|_| "Android direct-export state is poisoned".to_owned())?
        .remove(path)
        .ok_or_else(|| "Android direct-export destination is no longer available".to_owned())?;
    let DirectExportTarget {
        descriptor,
        uri,
        location,
        ..
    } = target;
    drop(descriptor);
    match finish_pending_export(app, &uri, true) {
        Ok(published) if !published.is_empty() => Ok(PublishedExport {
            location: published,
            uri,
        }),
        Ok(_) => Ok(PublishedExport { location, uri }),
        Err(error) => {
            let _ = finish_pending_export(app, &uri, false);
            Err(error)
        }
    }
}

pub fn cancel_direct_export(app: &AndroidApp, path: &Path) {
    let target = direct_exports()
        .lock()
        .ok()
        .and_then(|mut targets| targets.remove(path));
    if let Some(target) = target {
        let DirectExportTarget {
            descriptor, uri, ..
        } = target;
        drop(descriptor);
        if let Err(error) = finish_pending_export(app, &uri, false) {
            log::warn!("could not delete failed Android direct export: {error}");
        }
    }
}

pub fn cancel_all_direct_exports(app: &AndroidApp) {
    let targets = direct_exports()
        .lock()
        .map(|mut targets| {
            targets
                .drain()
                .map(|(_, target)| target)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for target in targets {
        let DirectExportTarget {
            descriptor, uri, ..
        } = target;
        drop(descriptor);
        if let Err(error) = finish_pending_export(app, &uri, false) {
            log::warn!("could not delete failed Android direct export: {error}");
        }
    }
}

/// Publishes (`success`) or deletes a pending MediaStore export. A published
/// export returns its final location, or an empty string when MediaStore
/// cannot report it.
fn finish_pending_export(app: &AndroidApp, uri: &str, success: bool) -> Result<String, String> {
    with_export_publisher(app, |env, export_publisher| {
        let uri = env.new_string(uri)?;
        let value = env.call_method(
            export_publisher,
            jni::jni_str!("finishPendingExport"),
            jni::jni_sig!((JString, i32) -> JString),
            &[
                JValue::Object(&uri),
                JValue::Int(if success { 1 } else { 0 }),
            ],
        )?;
        java_string(env, value)
    })
    .map_err(|error| format!("could not finalize Android MediaStore export: {error:#}"))
}

/// Opens the Android share sheet for a published export. The sheet opens
/// asynchronously on the Android UI thread.
pub fn share_export(app: &AndroidApp, uri: &str, mime_type: &str) -> Result<(), String> {
    with_export_publisher(app, |env, export_publisher| {
        let uri = env.new_string(uri)?;
        let mime_type = env.new_string(mime_type)?;
        env.call_method(
            export_publisher,
            jni::jni_str!("shareExport"),
            jni::jni_sig!((JString, JString) -> void),
            &[JValue::Object(&uri), JValue::Object(&mime_type)],
        )?;
        Ok(())
    })
    .map_err(|error| format!("could not open the Android share sheet: {error:#}"))
}

pub fn publish_image(
    app: &AndroidApp,
    path: &std::path::Path,
    display_name: &str,
    mime_type: &str,
) -> Result<(), String> {
    let path = path
        .to_str()
        .ok_or_else(|| "Android export cache path is not valid UTF-8".to_owned())?;
    with_export_publisher(app, |env, export_publisher| {
        let path = env.new_string(path)?;
        let display_name = env.new_string(display_name)?;
        let mime_type = env.new_string(mime_type)?;
        env.call_method(
            export_publisher,
            jni::jni_str!("publishImage"),
            jni::jni_sig!((JString, JString, JString) -> void),
            &[
                JValue::Object(&path),
                JValue::Object(&display_name),
                JValue::Object(&mime_type),
            ],
        )?;
        Ok(())
    })
    .map_err(|error| format!("could not publish Android image: {error:#}"))
}
