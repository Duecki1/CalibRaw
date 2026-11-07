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
            Err(error) => {
                self.media.pending_error = Some(error);
                return;
            }
        }
        let media = &mut self.media;
        let roots = std::mem::take(&mut media.trees);
        media.trees = roots
            .into_iter()
            .filter(|(key, _)| media.has_root(key))
            .collect();
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
        self.show_android_root(key, String::new(), context)
    }

    /// Shows `folder` of the root `key`, which need not be the root shown.
    /// Returns whether another folder is now shown.
    pub(crate) fn select_android_root_folder(
        &mut self,
        key: String,
        folder: String,
        context: &egui::Context,
    ) -> bool {
        if self.media.root == key {
            return self.select_android_folder(folder, context);
        }
        self.show_android_root(key, folder, context)
    }

    /// Shows another root at `folder`, or at its top when that folder cannot
    /// be selected.
    fn show_android_root(&mut self, key: String, folder: String, context: &egui::Context) -> bool {
        if let Err(error) = self.platform.select_library_root(&key) {
            self.media.pending_error = Some(error);
            return false;
        }
        let folder = if folder.is_empty() {
            folder
        } else {
            match self.platform.select_library_folder(&folder) {
                Ok(()) => folder,
                Err(error) => {
                    // The platform still lists from the root's top.
                    self.status = error;
                    String::new()
                }
            }
        };
        self.media.root = key;
        self.media.folder = folder;
        // The other root's walk is left to finish unseen; that root stays
        // unlisted and is walked again later.
        self.media.tree_walk = None;
        let selected_folder = self.media.folder.clone();
        self.media
            .shown_tree_mut()
            .expanded
            .extend(android_folder_ancestors(&selected_folder));
        self.media.root_location = self.platform.library_location().unwrap_or_else(|error| {
            log::warn!("{error}");
            String::new()
        });
        self.location = Some(android_library_location_label(
            &self.media.root_location,
            &selected_folder,
        ));
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
            .shown_tree_mut()
            .expanded
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
