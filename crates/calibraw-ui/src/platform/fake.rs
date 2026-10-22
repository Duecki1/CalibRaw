//! A platform for tests: chosen capabilities, scripted media-library
//! contents, and a record of the services the code under test called.

use super::service::{ImportedLibraryDocument, LibraryDocument, LibraryFolder, Platform};
use super::PlatformCaps;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub(crate) struct FakePlatform {
    caps: PlatformCaps,
    pub(crate) library_documents: Mutex<Vec<LibraryDocument>>,
    pub(crate) library_folders: Mutex<Vec<LibraryFolder>>,
    /// One line per service called, in order.
    pub(crate) calls: Mutex<Vec<String>>,
}

impl FakePlatform {
    pub(crate) fn new(caps: PlatformCaps) -> Arc<Self> {
        Arc::new(Self {
            caps,
            library_documents: Mutex::new(Vec::new()),
            library_folders: Mutex::new(Vec::new()),
            calls: Mutex::new(Vec::new()),
        })
    }

    pub(crate) fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }
}

impl Platform for FakePlatform {
    fn caps(&self) -> PlatformCaps {
        self.caps
    }

    fn release_free_memory(&self) {
        self.record("release_free_memory".to_owned());
    }

    fn performance_settings_path(&self) -> Result<Option<PathBuf>, String> {
        Ok(None)
    }

    fn model_root(&self) -> PathBuf {
        std::env::temp_dir().join("calibraw-test-models")
    }

    fn library_location(&self) -> Result<String, String> {
        Ok("Library".to_owned())
    }

    fn list_library_documents(&self) -> Result<Vec<LibraryDocument>, String> {
        Ok(self.library_documents.lock().unwrap().clone())
    }

    fn list_library_folders(&self) -> Result<Vec<LibraryFolder>, String> {
        Ok(self.library_folders.lock().unwrap().clone())
    }

    fn select_library_folder(&self, relative_path: &str) -> Result<(), String> {
        self.record(format!("select_library_folder {relative_path}"));
        Ok(())
    }

    fn open_library_document(&self, uri: &str, display_name: &str) -> Result<(), String> {
        self.record(format!("open_library_document {uri} {display_name}"));
        Ok(())
    }

    /// Records the imported name and size; the file is left where it was.
    fn import_local_library_document(
        &self,
        raw_path: &Path,
        display_name: &str,
    ) -> Result<ImportedLibraryDocument, String> {
        let bytes = std::fs::metadata(raw_path)
            .map_err(|error| error.to_string())?
            .len();
        self.record(format!(
            "import_local_library_document {display_name} {bytes}"
        ));
        Ok(ImportedLibraryDocument {
            uri: format!("content://calibraw.test/{display_name}"),
            display_name: display_name.to_owned(),
        })
    }
}
