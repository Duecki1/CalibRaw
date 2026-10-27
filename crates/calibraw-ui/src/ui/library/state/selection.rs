//! Which photos library actions apply to, and the photo the keyboard moves.
//!
//! The library is either browsing, where a click opens a photo and the
//! keyboard and review keys work on the active photo, or selecting, where
//! every thumbnail shows its check circle and a click adds or removes it.
//! Positions are into the grid's visible entry indices, which are ascending.

use super::*;

#[derive(Default)]
pub(crate) struct LibrarySelection {
    /// `None` while browsing.
    selecting: Option<Selecting>,
    /// The photo arrows move from, Enter opens and, while browsing, review
    /// keys change.
    active: Option<LibraryAssetId>,
    /// The open document the active photo last followed.
    followed_document: Option<LibraryAssetId>,
    /// Scroll the grid to the active photo on its next frame.
    reveal_active: bool,
    /// The keyboard moved the active photo, so touch layouts show it too.
    keyboard_used: bool,
}

#[derive(Default)]
struct Selecting {
    photos: HashSet<LibraryAssetId>,
    /// Where Shift ranges and long-press drags start.
    anchor: Option<LibraryAssetId>,
    /// While a long-press drag runs: the photos chosen before it began.
    drag_base: Option<HashSet<LibraryAssetId>>,
}

impl LibrarySelection {
    fn selecting(&self) -> Option<&Selecting> {
        self.selecting.as_ref()
    }

    /// The selection, entering selecting first when browsing.
    fn selecting_mut(&mut self) -> &mut Selecting {
        self.selecting.get_or_insert_with(Selecting::default)
    }
}

impl LibraryState {
    pub(crate) fn is_selecting(&self) -> bool {
        self.selection.selecting().is_some()
    }

    pub(crate) fn selected_count(&self) -> usize {
        self.selection
            .selecting()
            .map_or(0, |selecting| selecting.photos.len())
    }

    pub(in crate::ui::library) fn is_selected(&self, asset_id: &LibraryAssetId) -> bool {
        self.selection
            .selecting()
            .is_some_and(|selecting| selecting.photos.contains(asset_id))
    }

    pub(in crate::ui::library) fn active_asset_id(&self) -> Option<&LibraryAssetId> {
        self.selection.active.as_ref()
    }

    /// Whether thumbnails ring the active photo: always where a pointer
    /// moves, and on touch screens once the keyboard has moved it.
    pub(in crate::ui::library) fn shows_active_photo(&self, caps: PlatformCaps) -> bool {
        caps.is_desktop() || self.selection.keyboard_used
    }

    /// The active photo, while the grid shows it.
    pub(in crate::ui::library) fn active_asset(&self, visible: &[usize]) -> Option<LibraryAsset> {
        let position = self.active_position(visible)?;
        Some(self.entries[visible[position]].asset.clone())
    }

    /// The selected photos in display order.
    pub(crate) fn selected_assets(&self) -> Vec<LibraryAsset> {
        let Some(selecting) = self.selection.selecting() else {
            return Vec::new();
        };
        self.entries
            .iter()
            .filter(|entry| selecting.photos.contains(&entry.asset.id))
            .map(|entry| entry.asset.clone())
            .collect()
    }

    /// What keys apply to: the selection while selecting, otherwise the
    /// active photo the grid shows.
    pub(in crate::ui::library) fn target_assets(&self, visible: &[usize]) -> Vec<LibraryAsset> {
        if self.is_selecting() {
            self.selected_assets()
        } else {
            self.active_asset(visible).into_iter().collect()
        }
    }

    /// Starts selecting with nothing chosen yet.
    pub(crate) fn begin_selection(&mut self) {
        self.selection.selecting_mut();
    }

    /// Returns to browsing; the active photo stays.
    pub(crate) fn clear_selection(&mut self) {
        self.selection.selecting = None;
    }

    /// Forgets the selection and the active photo, for another folder.
    pub(in crate::ui::library) fn reset_selection(&mut self) {
        self.selection = LibrarySelection::default();
    }

    /// Adds `asset_id` to the selection, or removes it. Removing the last
    /// photo returns to browsing.
    pub(in crate::ui::library) fn toggle_selected(&mut self, asset_id: &LibraryAssetId) {
        let selecting = self.selection.selecting_mut();
        if !selecting.photos.remove(asset_id) {
            selecting.photos.insert(asset_id.clone());
        }
        selecting.anchor = Some(asset_id.clone());
        let emptied = selecting.photos.is_empty();
        self.selection.active = Some(asset_id.clone());
        if emptied {
            self.clear_selection();
        }
    }

    pub(in crate::ui::library) fn add_to_selection(&mut self, asset_id: &LibraryAssetId) {
        let selecting = self.selection.selecting_mut();
        selecting.photos.insert(asset_id.clone());
        selecting.anchor = Some(asset_id.clone());
        self.selection.active = Some(asset_id.clone());
    }

    /// Adds the photos from the anchor (or, while browsing, the active
    /// photo) through `asset_id` to the selection.
    pub(in crate::ui::library) fn select_range_to(
        &mut self,
        asset_id: &LibraryAssetId,
        visible: &[usize],
    ) {
        let Some(target) = self.visible_position(visible, asset_id) else {
            return;
        };
        let start_id = self
            .selection
            .selecting()
            .and_then(|selecting| selecting.anchor.as_ref())
            .or(self.selection.active.as_ref());
        let start = start_id
            .and_then(|id| self.visible_position(visible, id))
            .unwrap_or(target);
        let range = self.visible_ids(visible, start.min(target)..=start.max(target));
        let start_id = self.entries[visible[start]].asset.id.clone();
        let selecting = self.selection.selecting_mut();
        selecting.photos.extend(range);
        selecting.anchor.get_or_insert(start_id);
        self.selection.active = Some(asset_id.clone());
    }

    /// Selects every photo the grid shows.
    pub(in crate::ui::library) fn select_all(&mut self) {
        let visible = self.visible_entry_indices();
        let photos = self.visible_ids(&visible, 0..=visible.len().saturating_sub(1));
        let anchor = visible
            .first()
            .map(|index| self.entries[*index].asset.id.clone());
        let selecting = self.selection.selecting_mut();
        selecting.photos = photos;
        selecting.anchor = anchor;
    }

    /// Selects every photo the search and filters show; returns how many.
    pub(crate) fn select_search_matches(&mut self) -> usize {
        if !self.search_active() && !self.review_filter.active() {
            return 0;
        }
        self.select_all();
        self.selected_count()
    }

    /// Drops selected photos the grid no longer shows. The active photo
    /// stays, so it returns when a filter is cleared.
    pub(in crate::ui::library) fn retain_visible_selection(&mut self, visible: &[usize]) {
        let entry_indices = &self.entry_indices;
        if let Some(selecting) = &mut self.selection.selecting {
            let shown = |asset_id: &LibraryAssetId| {
                entry_indices
                    .get(asset_id)
                    .is_some_and(|index| visible.binary_search(index).is_ok())
            };
            selecting.photos.retain(|asset_id| shown(asset_id));
            if selecting
                .anchor
                .as_ref()
                .is_some_and(|anchor| !shown(anchor))
            {
                selecting.anchor = None;
            }
        }
    }

    /// Drops photos that left the library, after a rescan.
    pub(in crate::ui::library) fn forget_missing_selection(&mut self) {
        let entry_indices = &self.entry_indices;
        let present = |asset_id: &Option<LibraryAssetId>| {
            asset_id
                .as_ref()
                .is_none_or(|asset_id| entry_indices.contains_key(asset_id))
        };
        if !present(&self.selection.active) {
            self.selection.active = None;
        }
        if let Some(selecting) = &mut self.selection.selecting {
            selecting
                .photos
                .retain(|asset_id| entry_indices.contains_key(asset_id));
            if !present(&selecting.anchor) {
                selecting.anchor = None;
            }
        }
    }

    /// Makes the photo open in Develop the active one whenever another
    /// photo is opened, so the library returns to where editing left off.
    pub(in crate::ui::library) fn follow_document(&mut self, document: Option<LibraryAssetId>) {
        if self.selection.followed_document == document {
            return;
        }
        self.selection.followed_document = document.clone();
        if let Some(document) = document {
            self.selection.active = Some(document);
            self.selection.reveal_active = true;
        }
    }

    pub(in crate::ui::library) fn active_position(&self, visible: &[usize]) -> Option<usize> {
        self.visible_position(visible, self.selection.active.as_ref()?)
    }

    /// Moves the active photo to `position` with the keyboard; `extend`
    /// adds the photos on the way to the selection.
    pub(in crate::ui::library) fn activate(
        &mut self,
        visible: &[usize],
        position: usize,
        extend: bool,
    ) {
        let Some(index) = visible.get(position) else {
            return;
        };
        let asset_id = self.entries[*index].asset.id.clone();
        if extend {
            self.select_range_to(&asset_id, visible);
        }
        self.activate_id(asset_id);
    }

    /// Makes `asset_id` the active photo with the keyboard.
    pub(in crate::ui::library) fn activate_id(&mut self, asset_id: LibraryAssetId) {
        self.selection.active = Some(asset_id);
        self.selection.reveal_active = true;
        self.selection.keyboard_used = true;
    }

    /// The active photo's position when the grid should scroll to it.
    pub(in crate::ui::library) fn take_reveal_active(
        &mut self,
        visible: &[usize],
    ) -> Option<usize> {
        if !std::mem::take(&mut self.selection.reveal_active) {
            return None;
        }
        self.active_position(visible)
    }

    pub(in crate::ui::library) fn drag_selecting(&self) -> bool {
        self.selection
            .selecting()
            .is_some_and(|selecting| selecting.drag_base.is_some())
    }

    /// A long press: selects the photo and starts a drag that selects the
    /// photos between it and the finger.
    pub(in crate::ui::library) fn begin_drag_selection(&mut self, asset_id: &LibraryAssetId) {
        let selecting = self.selection.selecting_mut();
        selecting.drag_base = Some(selecting.photos.clone());
        selecting.photos.insert(asset_id.clone());
        selecting.anchor = Some(asset_id.clone());
        self.selection.active = Some(asset_id.clone());
    }

    pub(in crate::ui::library) fn drag_selection_to(
        &mut self,
        asset_id: &LibraryAssetId,
        visible: &[usize],
    ) {
        let Some(selecting) = self.selection.selecting() else {
            return;
        };
        let (Some(base), Some(anchor)) = (&selecting.drag_base, &selecting.anchor) else {
            return;
        };
        let (Some(start), Some(end)) = (
            self.visible_position(visible, anchor),
            self.visible_position(visible, asset_id),
        ) else {
            return;
        };
        let mut photos = base.clone();
        photos.extend(self.visible_ids(visible, start.min(end)..=start.max(end)));
        self.selection.selecting_mut().photos = photos;
        self.selection.active = Some(asset_id.clone());
    }

    pub(in crate::ui::library) fn end_drag_selection(&mut self) {
        if let Some(selecting) = &mut self.selection.selecting {
            selecting.drag_base = None;
        }
    }

    fn visible_position(&self, visible: &[usize], asset_id: &LibraryAssetId) -> Option<usize> {
        let index = self.entry_indices.get(asset_id)?;
        visible.binary_search(index).ok()
    }

    fn visible_ids(
        &self,
        visible: &[usize],
        positions: std::ops::RangeInclusive<usize>,
    ) -> HashSet<LibraryAssetId> {
        visible
            .get(positions)
            .unwrap_or_default()
            .iter()
            .map(|index| self.entries[*index].asset.id.clone())
            .collect()
    }
}
