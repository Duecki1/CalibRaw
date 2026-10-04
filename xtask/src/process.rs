//! Workspace paths, argument helpers and child-process execution shared by
//! the xtask commands.

use crate::{Result, XtaskError};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask must be located directly under the workspace root")
        .to_path_buf()
}

/// Resolves `path` against `root` unless it is already absolute.
pub(crate) fn rooted(root: &Path, path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

pub(crate) fn ensure_no_extra_args(args: &[OsString], command: &str) -> Result<()> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(XtaskError::usage(format!(
            "{command} does not accept arguments: {}",
            args.iter()
                .map(|arg| arg.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ")
        )))
    }
}

/// Advances `index` to the value that follows `option` and returns it.
pub(crate) fn next_value(args: &[OsString], index: &mut usize, option: &str) -> Result<OsString> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| XtaskError::usage(format!("{option} requires a value")))
}

pub(crate) fn find_executable(name: &str) -> Option<PathBuf> {
    let candidate = Path::new(name);
    if candidate.components().count() > 1 && candidate.is_file() {
        return Some(candidate.to_path_buf());
    }
    let path = env::var_os("PATH")?;
    for directory in env::split_paths(&path) {
        let direct = directory.join(name);
        if direct.is_file() {
            return Some(direct);
        }
        if cfg!(windows) {
            let executable = directory.join(format!("{name}.exe"));
            if executable.is_file() {
                return Some(executable);
            }
        }
    }
    None
}

pub(crate) fn require_executable(name: &str, message: &str) -> Result<PathBuf> {
    find_executable(name).ok_or_else(|| XtaskError::new(message))
}

pub(crate) fn executable_name(name: &str) -> OsString {
    if cfg!(windows) {
        OsString::from(format!("{name}.exe"))
    } else {
        OsString::from(name)
    }
}

/// Runs a command that prints its own diagnostics; a failure keeps its exit code.
pub(crate) fn run_checked(command: &mut Command, description: &str) -> Result<()> {
    let status = command
        .status()
        .map_err(|error| XtaskError::new(format!("unable to execute {description}: {error}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(XtaskError::silent(status.code().unwrap_or(1)))
    }
}

/// Runs a command and returns its stdout. A failure reports the captured stderr.
pub(crate) fn run_command_bytes(command: &mut Command, description: &str) -> Result<Vec<u8>> {
    let output = command
        .output()
        .map_err(|error| XtaskError::new(format!("could not execute {description}: {error}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(XtaskError::with_code(
            if stderr.is_empty() {
                format!("{description} failed")
            } else {
                format!("{description} failed: {stderr}")
            },
            output.status.code().unwrap_or(1),
        ));
    }
    Ok(output.stdout)
}

pub(crate) fn run_command_output(command: &mut Command, description: &str) -> Result<String> {
    String::from_utf8(run_command_bytes(command, description)?).map_err(|error| {
        XtaskError::new(format!("{description} produced non-UTF-8 output: {error}"))
    })
}

pub(crate) fn remove_path(path: &Path) -> Result<()> {
    if path.is_symlink() || path.is_file() {
        fs::remove_file(path)?;
    } else if path.is_dir() {
        fs::remove_dir_all(path)?;
    }
    Ok(())
}

pub(crate) fn require_file(path: &Path) -> Result<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(XtaskError::new(format!(
            "required file was not produced: {}",
            path.display()
        )))
    }
}

pub(crate) fn directory_has_extension(path: &Path, extension: &str) -> Result<bool> {
    if !path.is_dir() {
        return Ok(false);
    }
    let mut directories = vec![path.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                directories.push(entry.path());
            } else if file_type.is_file() && entry.path().extension() == Some(OsStr::new(extension))
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub(crate) fn temporary_directory(prefix: &str) -> Result<tempfile::TempDir> {
    tempfile::Builder::new()
        .prefix(&format!("{prefix}-"))
        .tempdir()
        .map_err(|error| XtaskError::new(format!("cannot create temporary directory: {error}")))
}

/// Recursively lists files below `root` whose extension is in `extensions`,
/// sorted by path. Hidden directories and `target`/`build` output are skipped.
pub(crate) fn source_files(root: &Path, extensions: &[&str]) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !root.exists() {
        return Ok(files);
    }
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory)
            .map_err(|error| XtaskError::new(format!("{}: {error}", directory.display())))?
        {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if file_type.is_dir() {
                if !name.starts_with('.') && name != "target" && name != "build" {
                    directories.push(path);
                }
            } else if file_type.is_file()
                && path
                    .extension()
                    .and_then(OsStr::to_str)
                    .is_some_and(|extension| extensions.contains(&extension))
            {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

/// `path` relative to `root` with forward slashes, for stable reports.
pub(crate) fn display_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}
