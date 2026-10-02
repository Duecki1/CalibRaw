use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_STAGING_ID: AtomicU64 = AtomicU64::new(0);

/// Replaces `path` with the contents produced by `write`, so readers see
/// either the previous file or the complete new one, never a partial write.
///
/// The content is staged in a hidden sibling, flushed, and renamed over the
/// destination. The staging file is removed on any failure. The parent
/// directory must already exist; it is deliberately not created, because a
/// missing folder (for example an unmounted volume) should fail the write.
pub fn write_atomically<E>(
    path: &Path,
    write: impl FnOnce(&mut File) -> Result<(), E>,
) -> Result<(), E>
where
    E: From<io::Error>,
{
    let parent = parent_directory(path);
    let staging = staging_path(path)?;
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)?;
        write(&mut file)?;
        file.sync_all()?;
        drop(file);
        replace_file(&staging, path)?;
        sync_parent_directory(parent)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staging);
    }
    result
}

/// [`write_atomically`] for application-owned files such as caches and
/// settings, creating missing parent directories first.
pub fn write_bytes_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    fs::create_dir_all(parent_directory(path))?;
    write_atomically(path, |file| file.write_all(bytes))
}

fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn staging_path(path: &Path) -> io::Result<PathBuf> {
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} has no file name", path.display()),
        )
    })?;
    let mut name = OsString::from(".");
    name.push(file_name);
    name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed)
    ));
    Ok(parent_directory(path).join(name))
}

#[cfg(not(windows))]
pub fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(windows)]
pub fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();

    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
pub fn sync_parent_directory(parent: &Path) -> std::io::Result<()> {
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
pub fn sync_parent_directory(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_directory(label: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "calibraw-file-ops-{label}-{}-{}",
            std::process::id(),
            NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&directory);
        directory
    }

    fn directory_entries(directory: &Path) -> Vec<OsString> {
        let mut entries = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        entries.sort();
        entries
    }

    #[test]
    fn atomic_write_replaces_existing_content_and_creates_parents() {
        let directory = test_directory("replace");
        let path = directory.join("nested").join("settings.json");
        write_bytes_atomically(&path, b"first").unwrap();
        write_bytes_atomically(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        assert_eq!(directory_entries(path.parent().unwrap()), ["settings.json"]);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn failed_atomic_write_keeps_the_original_and_removes_staging() {
        let directory = test_directory("failure");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("sidecar.json");
        fs::write(&path, b"original").unwrap();
        let result = write_atomically(&path, |file| {
            file.write_all(b"partial")?;
            Err(io::Error::other("encoder failed"))
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original");
        assert_eq!(directory_entries(&directory), ["sidecar.json"]);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn atomic_write_does_not_create_missing_directories() {
        let directory = test_directory("missing");
        let result = write_atomically(&directory.join("sidecar.json"), |file| file.write_all(b"x"));
        assert!(result.is_err());
        assert!(!directory.exists());
    }
}
