use super::super::*;

impl LibraryState {
    pub(crate) fn android_folder(&self) -> &str {
        &self.media.folder
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
        self.refresh(context);
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
