use super::super::*;

impl LibraryState {
    pub(crate) fn android_folder(&self) -> &str {
        &self.media.folder
    }

    /// The linked folder shown; empty for the media library itself.
    pub(crate) fn android_root(&self) -> &str {
        &self.media.root
    }

    /// The name of the linked folder shown; `None` in the media library.
    pub(crate) fn linked_folder_label(&self) -> Option<&str> {
        self.media
            .linked_folders
            .iter()
            .find(|folder| folder.key == self.media.root)
            .map(|folder| folder.label.as_str())
    }

    pub(crate) fn reload_linked_folders(&mut self) {
        match self.platform.list_linked_folders() {
            Ok(folders) => self.media.linked_folders = folders,
            Err(error) => self.media.pending_error = Some(error),
        }
    }

    pub(crate) fn take_linked_folder_error(&mut self) -> Option<String> {
        self.media.pending_error.take()
    }

    /// Shows the top of a linked folder, or of the media library for an empty
    /// key. Returns whether another folder is now shown.
    pub(crate) fn select_android_root(&mut self, key: String, context: &egui::Context) -> bool {
        if self.media.root == key && self.media.folder.is_empty() {
            self.refresh(context);
            return false;
        }
        if let Err(error) = self.platform.select_library_root(&key) {
            self.media.pending_error = Some(error);
            return false;
        }
        self.media.root = key;
        self.media.folder = String::new();
        self.media.folders.clear();
        // The other folder's walk is left to finish unseen.
        self.media.tree_walk = None;
        self.media.expanded_folders = android_folder_ancestors("");
        self.media.root_location = self.platform.library_location().unwrap_or_else(|error| {
            log::warn!("{error}");
            String::new()
        });
        self.location = Some(self.media.root_location.clone());
        self.reset_folder_contents();
        self.refresh(context);
        true
    }

    pub(crate) fn select_android_folder(
        &mut self,
        folder: String,
        context: &egui::Context,
    ) -> bool {
        if self.media.folder == folder {
            return false;
        }
        if let Err(error) = self.platform.select_library_folder(&folder) {
            self.status = error;
            return false;
        }
        self.media.folder = folder;
        let selected_folder = self.media.folder.clone();
        self.media
            .expanded_folders
            .extend(android_folder_ancestors(&selected_folder));
        let location = android_library_location_label(&self.media.root_location, &selected_folder);
        self.location = Some(location);
        self.reset_folder_contents();
        self.rescan(context);
        true
    }

    pub(crate) fn android_loading_thumbnail_for_uri(
        &mut self,
        uri: &str,
        context: &egui::Context,
    ) -> Option<(egui::TextureHandle, [u32; 2], Option<egui::Color32>)> {
        let asset_id = LibraryAssetId::Android(uri.to_owned());
        let index = self.entry_indices.get(&asset_id).copied()?;
        self.restore_resident_thumbnail_texture(index, context);
        self.loading_thumbnail_for_index(index)
    }
}
