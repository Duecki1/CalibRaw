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
        sync_file(&file)?;
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

/// `{stem}.{extension}`, or the first of `{stem}-2.{extension}`,
/// `{stem}-3.{extension}`, … that `is_taken` accepts, so an export never
/// proposes the name of an earlier one.
pub fn first_free_file_name(
    stem: &str,
    extension: &str,
    mut is_taken: impl FnMut(&str) -> bool,
) -> String {
    let mut name = format!("{stem}.{extension}");
    let mut index = 2usize;
    while is_taken(&name) {
        name = format!("{stem}-{index}.{extension}");
        index += 1;
    }
    name
}

/// Moves `source` into `directory` under the first name of
/// [`first_free_file_name`] that is still free at the moment of the move, and
/// returns the path it now has. An existing file is never replaced, even one
/// another program creates while the names are tried.
pub fn move_file_to_free_name(
    source: &Path,
    directory: &Path,
    stem: &str,
    extension: &str,
) -> io::Result<PathBuf> {
    let mut failure = None;
    let name = first_free_file_name(stem, extension, |name| {
        match move_file_without_replacing(source, &directory.join(name)) {
            Ok(()) => false,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => true,
            Err(error) => {
                failure = Some(error);
                false
            }
        }
    });
    match failure {
        Some(error) => Err(error),
        None => Ok(directory.join(name)),
    }
}

/// Moves `source` to `destination` in the same directory, failing with
/// [`io::ErrorKind::AlreadyExists`] instead of replacing an existing entry.
///
/// Linking the new name fails atomically when the name is taken; removing the
/// source name then completes the move. File systems without hard links (FAT,
/// exFAT, some network shares) fall back to checking the name just before
/// the rename, which a writer racing in between can still beat.
#[cfg(not(windows))]
pub fn move_file_without_replacing(source: &Path, destination: &Path) -> io::Result<()> {
    match fs::hard_link(source, destination) {
        Ok(()) => {
            // Both names refer to the complete file now. A leftover source
            // name is harmless; callers remove their staging file anyway.
            let _ = fs::remove_file(source);
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Err(error),
        Err(_) => match fs::symlink_metadata(destination) {
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} already exists", destination.display()),
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::rename(source, destination)
            }
            Err(error) => Err(error),
        },
    }
}

#[cfg(windows)]
pub fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    move_file_ex(
        source,
        destination,
        MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
    )
}

/// Moves `source` to `destination`, failing with
/// [`io::ErrorKind::AlreadyExists`] instead of replacing an existing entry.
/// Without `MOVEFILE_REPLACE_EXISTING` Windows checks and moves atomically on
/// every file system.
#[cfg(windows)]
pub fn move_file_without_replacing(source: &Path, destination: &Path) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH;
    move_file_ex(source, destination, MOVEFILE_WRITE_THROUGH)
}

#[cfg(windows)]
fn move_file_ex(
    source: &Path,
    destination: &Path,
    flags: windows_sys::Win32::Storage::FileSystem::MOVE_FILE_FLAGS,
) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

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

    // SAFETY: both buffers are NUL-terminated UTF-16 strings that outlive
    // the call, which only reads them.
    let moved = unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), flags) };
    if moved == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
pub fn sync_parent_directory(parent: &Path) -> std::io::Result<()> {
    sync_file(&File::open(parent)?)
}

#[cfg(not(unix))]
pub fn sync_parent_directory(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Flushes `file` to storage. Use this rather than [`File::sync_all`] for
/// anything that can live in a folder the user chose.
///
/// On Apple platforms `sync_all` is `fcntl(F_FULLFSYNC)`, which fails with
/// `ENOTSUP` on file systems that cannot honour it: SMB shares reject it for
/// directories, and on some servers (Samba) for files too. Those get a plain
/// `fsync`, the strongest flush they offer. Every other error is a real write
/// failure and is returned; retrying it with `fsync` could report success for
/// data that was lost.
#[cfg(target_vendor = "apple")]
pub fn sync_file(file: &File) -> io::Result<()> {
    fall_back_if_unsupported(file.sync_all(), || fsync(file))
}

#[cfg(not(target_vendor = "apple"))]
pub fn sync_file(file: &File) -> io::Result<()> {
    file.sync_all()
}

#[cfg(target_vendor = "apple")]
fn fall_back_if_unsupported(
    full_sync: io::Result<()>,
    fsync: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    match full_sync {
        Err(error) if error.raw_os_error() == Some(libc::ENOTSUP) => fsync(),
        result => result,
    }
}

#[cfg(target_vendor = "apple")]
fn fsync(file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    loop {
        // SAFETY: `file` keeps the descriptor open for the whole call, and
        // `fsync` only flushes it.
        if unsafe { libc::fsync(file.as_raw_fd()) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_file_flushes_files_and_directories() {
        let directory = tempfile::tempdir().expect("create directory");
        let path = directory.path().join("photo.calibraw");
        write_bytes_atomically(&path, b"edits").expect("write atomically");

        // Windows refuses to flush a handle opened read-only.
        let file = OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open file");
        sync_file(&file).expect("sync file");
        sync_parent_directory(directory.path()).expect("sync directory");
        assert_eq!(fs::read(&path).expect("read file"), b"edits");
    }

    #[cfg(target_vendor = "apple")]
    #[test]
    fn unsupported_full_sync_falls_back_to_fsync() {
        let unsupported = io::Error::from_raw_os_error(libc::ENOTSUP);
        let mut fell_back = false;
        let result = fall_back_if_unsupported(Err(unsupported), || {
            fell_back = true;
            Ok(())
        });
        assert!(result.is_ok());
        assert!(fell_back);
    }

    #[cfg(target_vendor = "apple")]
    #[test]
    fn failing_fallback_reports_the_fsync_error() {
        let unsupported = io::Error::from_raw_os_error(libc::ENOTSUP);
        let result = fall_back_if_unsupported(Err(unsupported), || {
            Err(io::Error::from_raw_os_error(libc::ENOSPC))
        });
        let error = result.expect_err("fsync failed");
        assert_eq!(error.raw_os_error(), Some(libc::ENOSPC));
    }

    #[cfg(target_vendor = "apple")]
    #[test]
    fn real_write_failures_do_not_fall_back() {
        let failure = io::Error::from_raw_os_error(libc::EIO);
        let mut fell_back = false;
        let result = fall_back_if_unsupported(Err(failure), || {
            fell_back = true;
            Ok(())
        });
        let error = result.expect_err("write failure stays an error");
        assert_eq!(error.raw_os_error(), Some(libc::EIO));
        assert!(!fell_back);
    }

    #[cfg(target_vendor = "apple")]
    #[test]
    fn fsync_flushes_an_open_file() {
        let file = tempfile::tempfile().expect("create file");
        fsync(&file).expect("fsync");
    }
}
