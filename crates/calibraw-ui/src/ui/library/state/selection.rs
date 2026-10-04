//! Thumbnail selection: single, range, all and search-match selection.

use super::*;

impl LibraryState {
    #[cfg(any(target_os = "android", test))]
    pub(crate) fn has_selection(&self) -> bool {
        !self.selected_assets.is_empty()
    }

    pub(crate) fn selection_mode(&self) -> bool {
        self.selection_mode
    }

    pub(crate) fn begin_selection(&mut self) {
        self.selection_mode = true;
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selected_assets.clear();
        self.selection_mode = false;
        self.selection_anchor = None;
    }

    pub(in crate::ui::library) fn retain_visible_selection(&mut self) {
        if self.selected_assets.is_empty() {
            return;
        }
        let visible = self
            .filtered_entry_indices()
            .into_iter()
            .map(|index| self.entries[index].asset.id.clone())
            .collect::<HashSet<_>>();
        self.selected_assets.retain(|id| visible.contains(id));
        if self.selected_assets.is_empty() {
            self.clear_selection();
        } else if self
            .selection_anchor
            .as_ref()
            .is_some_and(|id| !visible.contains(id))
        {
            self.selection_anchor = None;
        }
    }

    #[cfg(not(target_os = "android"))]
    pub(crate) fn select_search_matches(&mut self) -> usize {
        if !self.search_active() && !self.review_filter.active() {
            return 0;
        }
        self.select_all_thumbnails();

        self.selected_assets.len()
    }

    pub(in crate::ui::library) fn toggle_thumbnail_selection(
        &mut self,
        asset_id: &LibraryAssetId,
    ) -> bool {
        self.begin_selection();
        if !self.selected_assets.remove(asset_id) {
            self.selected_assets.insert(asset_id.clone());
            self.selection_anchor = Some(asset_id.clone());
        } else if self.selection_anchor.as_ref() == Some(asset_id) {
            self.selection_anchor = self.selected_assets.iter().next().cloned();
        }
        if self.selected_assets.is_empty() {
            self.clear_selection();
        }
        self.selection_mode()
    }

    #[cfg(not(target_os = "android"))]
    pub(in crate::ui::library) fn select_thumbnail(&mut self, asset_id: &LibraryAssetId) {
        self.begin_selection();
        self.selected_assets.insert(asset_id.clone());
        self.selection_anchor = Some(asset_id.clone());
    }

    #[cfg(not(target_os = "android"))]
    pub(in crate::ui::library) fn select_thumbnail_range(
        &mut self,
        asset_id: &LibraryAssetId,
        ordered_asset_ids: &[LibraryAssetId],
        extend_selection: bool,
    ) {
        let Some(anchor) = self.selection_anchor.as_ref() else {
            self.selected_assets.clear();
            self.select_thumbnail(asset_id);
            return;
        };
        let Some(anchor_index) = ordered_asset_ids.iter().position(|id| id == anchor) else {
            self.selected_assets.clear();
            self.select_thumbnail(asset_id);
            return;
        };
        let Some(target_index) = ordered_asset_ids.iter().position(|id| id == asset_id) else {
            return;
        };

        if !extend_selection {
            self.selected_assets.clear();
        }
        let (start, end) = if anchor_index <= target_index {
            (anchor_index, target_index)
        } else {
            (target_index, anchor_index)
        };
        self.selected_assets
            .extend(ordered_asset_ids[start..=end].iter().cloned());
        self.begin_selection();
    }

    #[cfg(not(target_os = "android"))]
    pub(in crate::ui::library) fn select_all_thumbnails(&mut self) {
        let indices = self.filtered_entry_indices();
        self.selected_assets = indices
            .iter()
            .map(|index| self.entries[*index].asset.id.clone())
            .collect();
        self.selection_anchor = indices
            .first()
            .map(|index| self.entries[*index].asset.id.clone());
        self.selection_mode = !self.selected_assets.is_empty();
    }
}
