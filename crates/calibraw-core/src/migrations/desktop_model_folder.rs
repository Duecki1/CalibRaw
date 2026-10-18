//! Desktop builds up to v1.3 downloaded AI models and automatic ONNX
//! Runtimes into a cache folder: `$XDG_CACHE_HOME/calibraw/models` or
//! `~/.cache/calibraw/models`, and where neither variable is set, as on
//! Windows, the temporary folder. Cleaners empty both, so the downloads now
//! live in the application data folder (`calibraw_ai::desktop_model_root`).
//! Moving them there on start-up spares a second download.
//!
//! migration: remove in v1.5.0

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The folder older desktop versions downloaded models and runtimes into.
pub fn legacy_desktop_model_root() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("calibraw/models")
}

/// Moves everything in `from` into `to`, then removes the folders it emptied.
///
/// - An entry `to` lacks is renamed into place. Where renaming fails, as
///   between two file systems, it is copied under a temporary name, renamed
///   into place and then removed from `from`.
/// - A folder both have is merged the same way. A file both have is kept in
///   `to` and removed from `from`.
/// - `keep`, a manually selected ONNX Runtime that the settings name by its
///   path, stays where it is, as do the folders that lead to it.
///
/// Does nothing when `from` does not exist or the two folders overlap. Every
/// entry is attempted; what fails stays in `from` for the next start, and
/// the first failure is returned.
pub fn move_desktop_models(from: &Path, to: &Path, keep: Option<&Path>) -> io::Result<()> {
    if !from.is_dir() || overlap(from, to) {
        return Ok(());
    }
    let keep = keep.and_then(|keep| path_below(keep, from));
    let mut first_error = None;
    merge(from, to, keep.as_deref(), &mut first_error);
    first_error.map_or(Ok(()), Err)
}

/// Whether one folder contains the other, also through symbolic links: a
/// file "both have" would then be one file, removed by the merge.
fn overlap(from: &Path, to: &Path) -> bool {
    let resolved = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let (from, to) = (resolved(from), resolved(to));
    from.starts_with(&to) || to.starts_with(&from)
}

/// `path` relative to `folder`, if it lies inside it.
fn path_below(path: &Path, folder: &Path) -> Option<PathBuf> {
    if let Ok(relative) = path.strip_prefix(folder) {
        return Some(relative.to_path_buf());
    }
    let path = fs::canonicalize(path).ok()?;
    let folder = fs::canonicalize(folder).ok()?;
    path.strip_prefix(folder).ok().map(Path::to_path_buf)
}

/// Moves the entries of the folder `from` into the folder `to`, leaving
/// `keep` (relative to `from`), and removes `from` once it is empty.
fn merge(from: &Path, to: &Path, keep: Option<&Path>, first_error: &mut Option<io::Error>) {
    let entries = match fs::read_dir(from) {
        Ok(entries) => entries,
        Err(error) => return record(first_error, from, error),
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                record(first_error, from, error);
                continue;
            }
        };
        let name = entry.file_name();
        let source = entry.path();
        let target = to.join(&name);
        match keep.and_then(|keep| keep.strip_prefix(&name).ok()) {
            // The kept file or folder itself.
            Some(below) if below.as_os_str().is_empty() => {}
            Some(below) => {
                if source.is_dir() {
                    merge(&source, &target, Some(below), first_error);
                }
            }
            None => move_entry(&source, &target, first_error),
        }
    }
    // Fails, as it should, while anything is left in `from`.
    let _ = fs::remove_dir(from);
}

fn move_entry(source: &Path, target: &Path, first_error: &mut Option<io::Error>) {
    let result = match fs::symlink_metadata(target) {
        Ok(existing) if existing.is_dir() && source.is_dir() => {
            return merge(source, target, None, first_error);
        }
        // `target` is what the current version uses.
        Ok(_) => remove(source),
        Err(error) if error.kind() == io::ErrorKind::NotFound => relocate(source, target),
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        record(first_error, source, error);
    }
}

/// Moves `source` to `target`, which does not exist yet.
fn relocate(source: &Path, target: &Path) -> io::Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::rename(source, target).is_ok() {
        return Ok(());
    }
    relocate_by_copy(source, target)
}

/// Copies `source` to `target` under a temporary name, so that a copy
/// interrupted part-way never looks like a finished download, and then
/// removes `source`. A temporary copy an interrupted run left is replaced.
fn relocate_by_copy(source: &Path, target: &Path) -> io::Result<()> {
    let mut staging_name = std::ffi::OsString::from(".");
    staging_name.push(target.file_name().unwrap_or_default());
    staging_name.push(".moving");
    let staging = target.with_file_name(staging_name);
    match remove(&staging) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    if let Err(error) = copy(source, &staging).and_then(|()| fs::rename(&staging, target)) {
        let _ = remove(&staging);
        return Err(error);
    }
    remove(source)
}

/// Copies a file, symbolic link or folder tree; links are copied as links.
fn copy(source: &Path, target: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.is_dir() {
        fs::create_dir(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy(&entry.path(), &target.join(entry.file_name()))?;
        }
        return Ok(());
    }
    #[cfg(unix)]
    if metadata.file_type().is_symlink() {
        return std::os::unix::fs::symlink(fs::read_link(source)?, target);
    }
    fs::copy(source, target).map(drop)
}

/// Removes a file, symbolic link or folder tree.
fn remove(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn record(first_error: &mut Option<io::Error>, path: &Path, error: io::Error) {
    log::warn!("could not move {}: {error}", path.display());
    first_error.get_or_insert_with(|| {
        io::Error::new(error.kind(), format!("{}: {error}", path.display()))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    #[test]
    fn moves_models_and_runtime_folders_and_removes_the_old_folder() {
        let root = tempfile::tempdir().unwrap();
        let from = root.path().join("cache/calibraw/models");
        let to = root.path().join("data/calibraw/models");
        write(&from.join("skyseg.onnx"), "sky");
        write(
            &from.join("onnxruntime/linux-x86_64-webgpu/lib/libonnxruntime.so"),
            "ort",
        );

        move_desktop_models(&from, &to, None).unwrap();

        assert_eq!(read(&to.join("skyseg.onnx")), "sky");
        assert_eq!(
            read(&to.join("onnxruntime/linux-x86_64-webgpu/lib/libonnxruntime.so")),
            "ort"
        );
        assert!(!from.exists());
        assert!(root.path().join("cache/calibraw").is_dir());
    }

    #[test]
    fn merges_into_an_existing_folder_and_keeps_its_files() {
        let root = tempfile::tempdir().unwrap();
        let from = root.path().join("old");
        let to = root.path().join("new");
        write(&from.join("depth.onnx"), "old depth");
        write(&from.join("onnxruntime/a/lib.so"), "a");
        write(&to.join("depth.onnx"), "new depth");
        write(&to.join("onnxruntime/b/lib.so"), "b");

        move_desktop_models(&from, &to, None).unwrap();

        assert_eq!(read(&to.join("depth.onnx")), "new depth");
        assert_eq!(read(&to.join("onnxruntime/a/lib.so")), "a");
        assert_eq!(read(&to.join("onnxruntime/b/lib.so")), "b");
        assert!(!from.exists());
    }

    #[test]
    fn leaves_the_manually_selected_runtime_and_moves_the_rest() {
        let root = tempfile::tempdir().unwrap();
        let from = root.path().join("old");
        let to = root.path().join("new");
        let manual = from.join("onnxruntime/linux-x86_64/lib/libonnxruntime.so");
        write(&manual, "manual");
        write(&from.join("onnxruntime/linux-x86_64/LICENSE"), "license");
        write(&from.join("onnxruntime/other/lib.so"), "other");
        write(&from.join("sam.onnx"), "sam");

        move_desktop_models(&from, &to, Some(&manual)).unwrap();

        assert_eq!(read(&manual), "manual");
        assert!(!to.join("onnxruntime/linux-x86_64/lib").exists());
        assert_eq!(
            read(&to.join("onnxruntime/linux-x86_64/LICENSE")),
            "license"
        );
        assert_eq!(read(&to.join("onnxruntime/other/lib.so")), "other");
        assert_eq!(read(&to.join("sam.onnx")), "sam");
        assert!(!from.join("sam.onnx").exists());
    }

    #[test]
    fn does_nothing_without_an_old_folder_or_when_the_folders_overlap() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("models");
        write(&folder.join("sky.onnx"), "sky");

        move_desktop_models(&root.path().join("missing"), &folder, None).unwrap();
        move_desktop_models(&folder, &folder, None).unwrap();
        move_desktop_models(&folder, &folder.join("nested"), None).unwrap();

        assert_eq!(read(&folder.join("sky.onnx")), "sky");
        assert!(!folder.join("nested").exists());
    }

    #[cfg(unix)]
    #[test]
    fn does_not_remove_files_when_the_new_folder_links_to_the_old_one() {
        let root = tempfile::tempdir().unwrap();
        let from = root.path().join("old");
        write(&from.join("sky.onnx"), "sky");
        let to = root.path().join("new");
        std::os::unix::fs::symlink(&from, &to).unwrap();

        move_desktop_models(&from, &to, None).unwrap();

        assert_eq!(read(&from.join("sky.onnx")), "sky");
    }

    #[test]
    fn copy_fallback_moves_a_tree_and_replaces_a_stale_staging_folder() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("old/onnxruntime");
        write(&source.join("lib/libonnxruntime.so.1.30.0"), "ort");
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            "libonnxruntime.so.1.30.0",
            source.join("lib/libonnxruntime.so"),
        )
        .unwrap();
        let target = root.path().join("new/onnxruntime");
        write(
            &root.path().join("new/.onnxruntime.moving/partial"),
            "partial",
        );

        relocate_by_copy(&source, &target).unwrap();

        assert_eq!(read(&target.join("lib/libonnxruntime.so.1.30.0")), "ort");
        #[cfg(unix)]
        assert!(fs::symlink_metadata(target.join("lib/libonnxruntime.so"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(!target.join("partial").exists());
        assert!(!root.path().join("new/.onnxruntime.moving").exists());
        assert!(!source.exists());
    }
}
