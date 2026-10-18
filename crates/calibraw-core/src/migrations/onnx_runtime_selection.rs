//! Desktop builds up to v1.3 saved the manually selected ONNX Runtime in
//! `$XDG_CONFIG_HOME/calibraw/onnx-runtime-path` or
//! `~/.config/calibraw/onnx-runtime-path`, and where neither variable is set,
//! as on Windows, in the temporary folder. The file now sits beside the
//! settings file, which on Linux is the same place.
//!
//! migration: remove in v2.0.0

use crate::file_ops::write_bytes_atomically;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Where older desktop versions saved the ONNX Runtime selection.
pub fn legacy_onnx_runtime_selection_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(std::env::temp_dir)
        .join("calibraw/onnx-runtime-path")
}

/// Moves the selection saved at `from` to `to`, unless there is none or `to`
/// already holds one.
pub fn move_onnx_runtime_selection(from: &Path, to: &Path) -> io::Result<()> {
    if from == to || to.exists() {
        return Ok(());
    }
    let selection = match fs::read(from) {
        Ok(selection) => selection,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    write_bytes_atomically(to, &selection)?;
    fs::remove_file(from)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SELECTION: &str = "sha256=00\npath=/opt/onnxruntime/libonnxruntime.so\n";

    #[test]
    fn moves_the_selection_beside_the_settings() {
        let root = tempfile::tempdir().unwrap();
        let from = root.path().join("temp/calibraw/onnx-runtime-path");
        let to = root.path().join("settings/calibraw/onnx-runtime-path");
        fs::create_dir_all(from.parent().unwrap()).unwrap();
        fs::write(&from, SELECTION).unwrap();

        move_onnx_runtime_selection(&from, &to).unwrap();

        assert_eq!(fs::read_to_string(&to).unwrap(), SELECTION);
        assert!(!from.exists());
    }

    #[test]
    fn keeps_a_selection_already_beside_the_settings() {
        let root = tempfile::tempdir().unwrap();
        let from = root.path().join("old");
        let to = root.path().join("new");
        fs::write(&from, SELECTION).unwrap();
        fs::write(&to, "current").unwrap();

        move_onnx_runtime_selection(&from, &to).unwrap();
        move_onnx_runtime_selection(&to, &to).unwrap();
        move_onnx_runtime_selection(&root.path().join("missing"), &root.path().join("none"))
            .unwrap();

        assert_eq!(fs::read_to_string(&to).unwrap(), "current");
        assert!(!root.path().join("none").exists());
    }
}
