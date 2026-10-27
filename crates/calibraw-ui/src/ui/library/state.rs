use super::*;

mod reviews;
mod scan;
mod selection;

pub(crate) use reviews::ReviewSave;
pub(super) use reviews::ReviewWriter;
pub(super) use selection::LibrarySelection;

/// Library settings restored from the persisted performance settings.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LibraryPreferences {
    /// The manual worker limit, used while `automatic_thumbnail_workers` is off.
    pub(crate) thumbnail_workers: usize,
    pub(crate) automatic_thumbnail_workers: bool,
    pub(crate) thumbnail_size: LibraryThumbnailSize,
    pub(crate) sort_order: LibrarySortOrder,
    pub(crate) stack_raw_companions: bool,
    pub(crate) render_edited_thumbnails_during_indexing: bool,
}

impl LibraryPreferences {
    /// Clamps the manual worker limit and applies the active worker count to
    /// the shared rendered-thumbnail limit.
    fn thumbnail_workers(self) -> usize {
        let thumbnail_workers = self
            .thumbnail_workers
            .clamp(1, maximum_thumbnail_worker_count());
        apply_rendered_thumbnail_limit(active_thumbnail_worker_count(
            self.automatic_thumbnail_workers,
            thumbnail_workers,
        ));
        thumbnail_workers
    }
}

fn active_thumbnail_worker_count(automatic: bool, manual: usize) -> usize {
    if automatic {
        automatic_thumbnail_worker_count()
    } else {
        manual
    }
}

fn apply_rendered_thumbnail_limit(active_workers: usize) {
    calibraw_core::thumbnail_cache::set_rendered_thumbnail_worker_limit(
        active_workers.min(MAX_RENDERED_THUMBNAIL_WORKERS),
    );
}

impl LibraryState {
    /// A library of folders; none is open until the person picks one.
    pub(crate) fn new_desktop(
        platform: Arc<dyn Platform>,
        preferences: LibraryPreferences,
        folder_sidebar_open: bool,
    ) -> Self {
        Self::with_preferences(
            preferences,
            None,
            folder_sidebar_open,
            "Open a folder to build your photo library.".to_owned(),
            platform,
            MediaLibraryState::default(),
        )
    }

    /// The platform's media library, scanned from `selected_folder`.
    pub(crate) fn new_android(
        platform: Arc<dyn Platform>,
        context: &egui::Context,
        preferences: LibraryPreferences,
        selected_folder: String,
    ) -> Self {
        let root_location = platform.library_location().unwrap_or_else(|error| {
            log::warn!("{error}");
            "Android/media/de.duecki.calibraw/.library".to_owned()
        });
        let selected_folder = match platform.select_library_folder(&selected_folder) {
            Ok(()) => selected_folder,
            Err(error) => {
                log::warn!("{error}");
                if let Err(root_error) = platform.select_library_folder("") {
                    log::warn!("{root_error}");
                }
                String::new()
            }
        };
        let location = android_library_location_label(&root_location, &selected_folder);
        let mut state = Self::with_preferences(
            preferences,
            Some(location),
            false,
            String::new(),
            platform,
            MediaLibraryState {
                root_location,
                folder: selected_folder.clone(),
                folders: Vec::new(),
                expanded_folders: android_folder_ancestors(&selected_folder),
                folder_name_dialog: None,
            },
        );
        state.refresh(context);
        state
    }

    /// A library with nothing scanned yet, shared by both kinds of storage.
    fn with_preferences(
        preferences: LibraryPreferences,
        location: Option<String>,
        folder_sidebar_open: bool,
        status: String,
        platform: Arc<dyn Platform>,
        media: MediaLibraryState,
    ) -> Self {
        let LibraryPreferences {
            thumbnail_size,
            sort_order,
            stack_raw_companions,
            render_edited_thumbnails_during_indexing,
            automatic_thumbnail_workers,
            ..
        } = preferences;
        let thumbnail_workers = preferences.thumbnail_workers();
        Self {
            location,
            folder: None,
            root_folder: None,
            folder_tree: None,
            expanded_folders: HashSet::new(),
            folder_sidebar_open,
            platform,
            media,
            entries: Vec::new(),
            entry_indices: HashMap::new(),
            event_receiver: None,
            request_sender: None,
            generation: Arc::new(AtomicU64::new(0)),
            decoding_paused: Arc::new(AtomicBool::new(false)),
            decode_gate: Arc::new(RwLock::new(())),
            thumbnail_progress: ThumbnailBackgroundProgress::default(),
            scanning: false,
            catalog_ready: false,
            status,
            usage_clock: 0,
            thumbnail_workers,
            automatic_thumbnail_workers,
            render_edited_thumbnails_during_indexing,
            sort_order,
            thumbnail_size,
            search_query: String::new(),
            review_filter: LibraryReviewFilter::default(),
            stack_raw_companions,
            raw_companions: RawCompanions::default(),
            selection: LibrarySelection::default(),
            reviews: ReviewWriter::default(),
            image_clipboard: None,
            adjustment_clipboard: None,
            asset_transfer_receiver: None,
            merge: None,
            raw_import_receiver: None,
            folder_operation_receiver: None,
            folder_clipboard: None,
            folder_name_dialog: None,
            folder_delete_confirmation: None,
            delete_originals_confirmation: None,
            raw_name_dialog: None,
            export_dialog: None,
            adjustment_paste_dialog: None,
            ai_mask_refresh_prompt: None,
        }
    }

    pub(crate) fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
    }

    pub(crate) fn has_copied_adjustments(&self) -> bool {
        self.adjustment_clipboard.is_some()
    }

    pub(crate) fn install_adjustment_clipboard(
        &mut self,
        edits: crate::sidecar::EditState,
        selection: crate::sidecar::EditSelection,
    ) {
        self.adjustment_clipboard = Some(AdjustmentClipboard { edits, selection });
    }

    pub(crate) fn asset_transfer_in_progress(&self) -> bool {
        self.asset_transfer_receiver.is_some()
    }

    pub(crate) fn transient_dialog_open(&self) -> bool {
        let common = self.delete_originals_confirmation.is_some()
            || self.raw_name_dialog.is_some()
            || self.export_dialog.is_some()
            || self.adjustment_paste_dialog.is_some()
            || self.ai_mask_refresh_prompt.is_some();
        common
            || self.folder_name_dialog.is_some()
            || self.folder_delete_confirmation.is_some()
            || self.media.folder_name_dialog.is_some()
    }

    pub(crate) fn local_mutation_in_progress(&self) -> bool {
        if self.asset_transfer_in_progress() {
            return true;
        }
        self.raw_import_receiver.is_some()
            || self.folder_operation_receiver.is_some()
            || self.merge.is_some()
    }

    pub(crate) fn search_query_mut(&mut self) -> &mut String {
        &mut self.search_query
    }

    pub(crate) fn clear_search(&mut self) {
        self.search_query.clear();
    }

    pub(crate) fn search_active(&self) -> bool {
        !library_search_terms(&self.search_query).is_empty()
    }

    /// Entry indices the grid shows, ascending.
    pub(super) fn visible_entry_indices(&self) -> Vec<usize> {
        let terms = library_search_terms(&self.search_query);
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                (library_filename_matches(&entry.asset.display_name, &terms)
                    && self.review_filter.matches(entry.review())
                    && !self.hides_raw_companion(&entry.asset.id))
                .then_some(index)
            })
            .collect()
    }

    /// Photos in the folder, counting each stacked RAW+JPEG pair once.
    pub(super) fn photo_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| !self.hides_raw_companion(&entry.asset.id))
            .count()
    }

    pub(crate) fn stacks_raw_companions(&self) -> bool {
        self.stack_raw_companions
    }

    pub(crate) fn set_stack_raw_companions(&mut self, stack: bool) -> bool {
        if self.stack_raw_companions == stack {
            return false;
        }
        self.stack_raw_companions = stack;
        self.retain_visible_selection(&self.visible_entry_indices());
        true
    }

    pub(super) fn hides_raw_companion(&self, asset: &LibraryAssetId) -> bool {
        self.stack_raw_companions && self.raw_companions.is_companion(asset)
    }

    /// Rendered formats stacked under the RAW `asset`, for its hover details.
    pub(super) fn stacked_formats(
        &self,
        asset: &LibraryAssetId,
    ) -> &[crate::pipeline::RenderedImageFormat] {
        if self.stack_raw_companions {
            self.raw_companions.paired_formats(asset)
        } else {
            &[]
        }
    }

    /// The manual worker limit, kept while the count is automatic.
    pub(crate) fn thumbnail_worker_count(&self) -> usize {
        self.thumbnail_workers
    }

    pub(crate) fn automatic_thumbnail_workers(&self) -> bool {
        self.automatic_thumbnail_workers
    }

    /// Workers the next indexing pass starts.
    pub(crate) fn active_thumbnail_worker_count(&self) -> usize {
        active_thumbnail_worker_count(self.automatic_thumbnail_workers, self.thumbnail_workers)
    }

    /// Whether a folder scan is running; thumbnails load after it finishes.
    pub(crate) fn is_scanning(&self) -> bool {
        self.scanning
    }

    pub(crate) fn renders_edited_thumbnails_during_indexing(&self) -> bool {
        self.render_edited_thumbnails_during_indexing
    }

    pub(crate) fn set_render_edited_thumbnails_during_indexing(
        &mut self,
        enabled: bool,
        context: &egui::Context,
    ) -> bool {
        if self.render_edited_thumbnails_during_indexing == enabled {
            return false;
        }
        self.render_edited_thumbnails_during_indexing = enabled;
        self.refresh(context);
        true
    }

    pub(crate) fn thumbnail_background_progress(&self) -> Option<ThumbnailProgress> {
        self.thumbnail_progress
            .snapshot(self.decoding_paused.load(Ordering::Acquire))
    }

    pub(crate) fn thumbnail_size(&self) -> LibraryThumbnailSize {
        self.thumbnail_size
    }

    pub(crate) fn set_thumbnail_size(&mut self, thumbnail_size: LibraryThumbnailSize) -> bool {
        if self.thumbnail_size == thumbnail_size {
            return false;
        }
        self.thumbnail_size = thumbnail_size;
        true
    }

    pub(crate) fn sort_order(&self) -> LibrarySortOrder {
        self.sort_order
    }

    pub(crate) fn set_sort_order(&mut self, sort_order: LibrarySortOrder) -> bool {
        if self.sort_order == sort_order {
            return false;
        }
        self.sort_order = sort_order;
        self.sort_entries();
        true
    }

    /// Empties the grid for another folder until its scan arrives.
    pub(super) fn reset_folder_contents(&mut self) {
        self.entries.clear();
        self.entry_indices.clear();
        self.reset_selection();
        self.catalog_ready = false;
    }

    fn sorts_by_review(&self) -> bool {
        matches!(
            self.sort_order,
            LibrarySortOrder::RatingHighestFirst
                | LibrarySortOrder::RatingLowestFirst
                | LibrarySortOrder::FlagPickedFirst
                | LibrarySortOrder::FlagRejectedFirst
        )
    }

    /// The photo's review as the library shows it.
    pub(crate) fn review_of(
        &self,
        asset_id: &LibraryAssetId,
    ) -> Option<crate::sidecar::PhotoReview> {
        let index = *self.entry_indices.get(asset_id)?;
        Some(self.entries[index].review())
    }

    /// Shows `review` for the photo; returns whether that changed the order
    /// the grid sorts by.
    pub(super) fn show_review(
        &mut self,
        asset_id: &LibraryAssetId,
        review: crate::sidecar::PhotoReview,
    ) -> bool {
        let Some(index) = self.entry_indices.get(asset_id).copied() else {
            return false;
        };
        let metadata = &mut self.entries[index].asset.metadata;
        let changed = metadata.review != review;
        metadata.review = review;
        changed && self.sorts_by_review()
    }

    /// Shows a review saved outside the library, re-sorting when needed.
    pub(crate) fn show_saved_review(
        &mut self,
        asset_id: &LibraryAssetId,
        review: crate::sidecar::PhotoReview,
    ) {
        if self.show_review(asset_id, review) {
            self.sort_entries();
        }
    }

    pub(super) fn sort_entries(&mut self) {
        let sort_order = self.sort_order;
        self.entries
            .sort_by(|left, right| compare_library_entries(left, right, sort_order));
        self.rebuild_entry_indices();
    }

    pub(super) fn rebuild_entry_indices(&mut self) {
        self.entry_indices = self
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.asset.id.clone(), index))
            .collect();
        self.raw_companions = RawCompanions::index(self.entries.iter().map(|entry| &entry.asset));
    }

    pub(crate) fn set_thumbnail_worker_count(&mut self, workers: usize, context: &egui::Context) {
        let workers = workers.clamp(1, maximum_thumbnail_worker_count());
        if self.thumbnail_workers == workers {
            return;
        }
        let previous_active = self.active_thumbnail_worker_count();
        self.thumbnail_workers = workers;
        self.restart_if_active_workers_changed(previous_active, context);
    }

    pub(crate) fn set_automatic_thumbnail_workers(
        &mut self,
        automatic: bool,
        context: &egui::Context,
    ) {
        if self.automatic_thumbnail_workers == automatic {
            return;
        }
        let previous_active = self.active_thumbnail_worker_count();
        self.automatic_thumbnail_workers = automatic;
        self.restart_if_active_workers_changed(previous_active, context);
    }

    fn restart_if_active_workers_changed(
        &mut self,
        previous_active: usize,
        context: &egui::Context,
    ) {
        let active = self.active_thumbnail_worker_count();
        if active == previous_active {
            return;
        }
        apply_rendered_thumbnail_limit(active);
        if self.location.is_some() {
            self.refresh(context);
        }
    }

    pub(crate) fn prepare_for_develop(&mut self) {
        self.decoding_paused.store(true, Ordering::Release);
        if self.platform.caps().constrained_memory() {
            self.evict_textures_to_limit(ANDROID_DEVELOP_TEXTURE_CACHE_LIMIT);
        }
    }

    pub(crate) fn decode_gate(&self) -> Arc<RwLock<()>> {
        Arc::clone(&self.decode_gate)
    }

    pub(crate) fn prepare_for_thumbnail_cache_clear(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.event_receiver = None;
        self.request_sender = None;
        self.thumbnail_progress = ThumbnailBackgroundProgress::default();
        self.scanning = false;
        self.usage_clock = 0;
        for entry in &mut self.entries {
            entry.texture = None;
            entry.resident_thumbnail = None;
            entry.texture_is_resident = false;
            entry.thumbnail_size = None;
            entry.thumbnail_error = None;
            entry.thumbnail_failures = 0;
            entry.thumbnail_retry_after = None;
            entry.thumbnail_queued = false;
            entry.developed_thumbnail = false;
            entry.developed_thumbnail_pending = false;
            entry.last_used = 0;
        }
    }

    pub(super) fn resume_thumbnail_decoding(&self) {
        self.decoding_paused.store(false, Ordering::Release);
    }
}

pub(super) fn library_search_terms(query: &str) -> Vec<String> {
    query
        .split(',')
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(str::to_lowercase)
        .collect()
}

pub(super) fn library_filename_matches(display_name: &str, terms: &[String]) -> bool {
    if terms.is_empty() {
        return true;
    }
    let display_name = display_name.to_lowercase();
    terms.iter().any(|term| display_name.contains(term))
}
