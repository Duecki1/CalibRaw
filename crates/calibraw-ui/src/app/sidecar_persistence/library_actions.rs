//! Detaching or resetting the open document before library file operations.

use super::*;

impl CalibRawApp {
    #[cfg(not(target_os = "android"))]
    pub(crate) fn detach_current_file_for_library_action(
        &mut self,
        raw_path: &std::path::Path,
    ) -> bool {
        if self.develop.current_path.as_deref() != Some(raw_path) {
            return false;
        }
        self.detach_current_sidecar_target_for_library_action()
    }

    #[cfg(target_os = "android")]
    pub(crate) fn detach_current_android_document_for_library_action(
        &mut self,
        raw_uri: &str,
        display_name: &str,
    ) -> bool {
        let is_current = matches!(
            self.persistence.sidecar_target.as_ref(),
            Some(crate::sidecar::SidecarTarget::Android {
                raw_uri: current_uri,
                display_name: current_name,
            }) if current_uri == raw_uri && current_name == display_name
        );
        if !is_current {
            return false;
        }
        self.detach_current_sidecar_target_for_library_action()
    }

    #[cfg(target_os = "android")]
    pub(crate) fn reset_android_library_adjustments(
        &mut self,
        raw_uri: &str,
        display_name: &str,
    ) -> Result<(), String> {
        let was_current =
            self.detach_current_android_document_for_library_action(raw_uri, display_name);
        let result = if was_current {
            calibraw_ffi::reset_android_adjustments_with_editing_time(
                &self.android.android_app,
                raw_uri,
                display_name,
                self.raw_editing_time_ms(),
            )
        } else {
            calibraw_ffi::reset_android_adjustments(
                &self.android.android_app,
                raw_uri,
                display_name,
            )
        };
        if was_current && result.is_ok() {
            self.reload_android_library_document_after_reset(raw_uri, display_name);
        }
        result
    }

    #[cfg(target_os = "android")]
    pub(crate) fn rename_android_library_item(
        &mut self,
        raw_uri: &str,
        display_name: &str,
        requested_name: &str,
    ) -> Result<String, String> {
        let was_current =
            self.detach_current_android_document_for_library_action(raw_uri, display_name);
        let result = calibraw_ffi::rename_library_document(
            &self.android.android_app,
            raw_uri,
            display_name,
            requested_name,
        );
        match result {
            Ok(renamed_uri) => {
                if was_current {
                    self.open_android_library_document(&renamed_uri, requested_name);
                }
                Ok(renamed_uri)
            }
            Err(error) => {
                if was_current {
                    self.open_android_library_document(raw_uri, display_name);
                }
                Err(error)
            }
        }
    }

    #[cfg(target_os = "android")]
    pub(crate) fn delete_android_library_item(
        &mut self,
        raw_uri: &str,
        display_name: &str,
    ) -> Result<(), String> {
        let was_current =
            self.detach_current_android_document_for_library_action(raw_uri, display_name);
        let result =
            calibraw_ffi::delete_library_document(&self.android.android_app, raw_uri, display_name);
        if result.is_err() && was_current {
            self.open_android_library_document(raw_uri, display_name);
        }
        result
    }

    pub(in crate::app) fn detach_current_sidecar_target_for_library_action(&mut self) -> bool {
        self.flush_sidecar_on_exit();
        let detached_generation = self.persistence.document_generation;
        self.persistence.document_generation = self.persistence.document_generation.wrapping_add(1);
        self.persistence.sidecar_target = None;
        self.persistence.sidecar_saved_revision = None;
        self.persistence.sidecar_failed_revision = None;
        self.persistence.sidecar_autosave_deadline = None;
        self.persistence
            .sidecar_pending
            .retain(|request| request.generation != detached_generation);
        true
    }
}
