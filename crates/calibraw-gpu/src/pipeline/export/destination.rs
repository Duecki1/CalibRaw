//! Export destinations: staging files, descriptor targets and atomic publishing.

use super::*;

/// Where an export writer puts its file and its intermediate staging files.
#[derive(Clone, Copy, Debug)]
pub(super) struct ExportOutput<'a> {
    /// The file the writer produces.
    pub(super) path: &'a Path,
    /// `path` is an existing platform descriptor to truncate rather than a
    /// new file to create exclusively.
    pub(super) truncate_existing: bool,
    pub(super) staging_dir: &'a Path,
    /// Name prefix of staged intermediates (see [`temporary_export_path`]).
    pub(super) staging_name: &'a str,
}

/// Name prefix of staged files for descriptor targets, whose paths carry no name.
const DESCRIPTOR_STAGING_NAME: &str = "calibraw-direct-export";

pub(super) fn export_to_destination<F>(
    target: &ExportTarget,
    cancellation: &AtomicBool,
    export: F,
) -> Result<()>
where
    F: FnOnce(ExportOutput<'_>) -> Result<()>,
{
    ensure_export_not_cancelled(cancellation)?;
    match target {
        ExportTarget::Descriptor { path, staging_dir } => {
            export(ExportOutput {
                path,
                truncate_existing: true,
                staging_dir,
                staging_name: DESCRIPTOR_STAGING_NAME,
            })?;
            ensure_export_not_cancelled(cancellation)
        }
        ExportTarget::File(destination) => {
            let staging_dir = parent_directory(destination);
            let name = file_name(destination)?;
            let temporary = temporary_export_path(staging_dir, name)?;
            let result = (|| {
                export(ExportOutput {
                    path: &temporary,
                    truncate_existing: false,
                    staging_dir,
                    staging_name: file_name(&temporary)?,
                })?;
                ensure_export_not_cancelled(cancellation)?;
                publish_completed_export(&temporary, destination)
            })();
            let _ = fs::remove_file(&temporary);
            result
        }
    }
}

pub(super) fn open_export_destination(output: ExportOutput<'_>) -> Result<fs::File> {
    let mut options = OpenOptions::new();
    options.write(true);
    if output.truncate_existing {
        options.truncate(true);
    } else {
        options.create_new(true);
    }
    options
        .open(output.path)
        .with_context(|| format!("open export destination {}", output.path.display()))
}

fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn file_name(path: &Path) -> Result<&str> {
    path.file_name()
        .and_then(|value| value.to_str())
        .context("export path has no valid file name")
}

/// A new unique `.<name>.<pid>.<nonce>.<id>.part` path in `directory`, after
/// removing day-old parts that earlier interrupted exports of `name` left.
fn temporary_export_path(directory: &Path, name: &str) -> Result<PathBuf> {
    fs::create_dir_all(directory)
        .with_context(|| format!("create export directory {}", directory.display()))?;
    cleanup_stale_export_parts(directory, name);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary_id = NEXT_EXPORT_TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
    Ok(directory.join(format!(
        ".{name}.{}.{}.{}.part",
        std::process::id(),
        nonce,
        temporary_id
    )))
}

/// Runs `action` with a staged intermediate file for `output`, which is
/// removed afterwards whatever the outcome.
pub(super) fn with_staging_file<T, F>(output: ExportOutput<'_>, action: F) -> Result<T>
where
    F: FnOnce(&Path) -> Result<T>,
{
    let temporary = temporary_export_path(output.staging_dir, output.staging_name)?;
    let result = action(&temporary);
    let _ = fs::remove_file(&temporary);
    result
}

fn cleanup_stale_export_parts(parent: &Path, destination_name: &str) {
    let prefix = format!(".{destination_name}.");
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(&prefix) || !name.ends_with(".part") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let old_enough = metadata
            .modified()
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age >= STALE_EXPORT_PART_AGE);
        if old_enough {
            let _ = fs::remove_file(entry.path());
        }
    }
}

pub(super) fn publish_completed_export(temporary: &Path, destination: &Path) -> Result<()> {
    OpenOptions::new()
        .write(true)
        .open(temporary)
        .with_context(|| format!("open completed export {}", temporary.display()))?
        .sync_all()
        .with_context(|| format!("flush completed export {}", temporary.display()))?;

    replace_file(temporary, destination).with_context(|| {
        format!(
            "publish completed export {} to {}",
            temporary.display(),
            destination.display()
        )
    })?;

    let parent = parent_directory(destination);
    sync_parent_directory(parent)
        .with_context(|| format!("flush export directory {}", parent.display()))
}
