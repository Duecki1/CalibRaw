//! A platform for tests: chosen capabilities, scripted media-library
//! contents, and a record of the services the code under test called.

use super::service::{LibraryDocument, LibraryFolder, Platform};
use super::PlatformCaps;
use std::path::PathBuf;
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
}
