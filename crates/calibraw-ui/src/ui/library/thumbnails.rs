use super::*;

mod developed;
mod loaders;
mod workers;
pub(super) use developed::*;
pub(super) use loaders::*;
#[cfg(not(target_os = "android"))]
pub(crate) use loaders::{load_desktop_cached_thumbnail, load_desktop_reference_preview};
pub(super) use workers::*;

impl LibraryState {
    pub(crate) fn touch_and_request_thumbnail(&mut self, index: usize, context: &egui::Context) {
        self.restore_resident_thumbnail_texture(index, context);

        let generation = self.generation.load(Ordering::Acquire);
        let request_sender = self.request_sender.clone();

        let Some(entry) = self.entries.get_mut(index) else {
            return;
        };

        if entry.texture.is_some() && !entry.texture_is_resident || entry.thumbnail_queued {
            return;
        }
        if entry.thumbnail_error.is_some() {
            if entry
                .thumbnail_retry_after
                .is_some_and(|retry_after| Instant::now() < retry_after)
            {
                return;
            }
            entry.thumbnail_error = None;
            entry.thumbnail_retry_after = None;
        }
        let request = ThumbnailRequest {
            generation,
            asset_id: entry.asset.id.clone(),
            display_priority: true,
            stage: ThumbnailLoadStage::RawPreview,
        };
        if request_sender
            .as_ref()
            .is_some_and(|sender| sender.try_send(request).is_ok())
        {
            entry.thumbnail_queued = true;
        }
    }

    pub(super) fn restore_resident_thumbnail_texture(
        &mut self,
        index: usize,
        context: &egui::Context,
    ) {
        let generation = self.generation.load(Ordering::Acquire);
        self.usage_clock = self.usage_clock.wrapping_add(1).max(1);
        let usage_clock = self.usage_clock;
        let Some(entry) = self.entries.get_mut(index) else {
            return;
        };
        entry.last_used = usage_clock;

        if entry.texture.is_none() {
            if let Some(resident) = entry.resident_thumbnail.as_ref() {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [resident.width as usize, resident.height as usize],
                    &resident.rgba,
                );
                entry.texture = Some(context.load_texture(
                    format!("library-resident-thumbnail-{generation}-{index}-{usage_clock}"),
                    image,
                    egui::TextureOptions::LINEAR,
                ));
                entry.texture_is_resident = entry
                    .thumbnail_size
                    .is_some_and(|size| size != [resident.width, resident.height]);
            }
        }
    }

    #[cfg(not(target_os = "android"))]
    pub(crate) fn install_developed_thumbnail(
        &mut self,
        raw_path: &Path,
        thumbnail: RawThumbnail,
        context: &egui::Context,
        revision: u64,
    ) {
        let asset_id = LibraryAssetId::Desktop(raw_path.to_owned());
        let Some(index) = self.entry_indices.get(&asset_id).copied() else {
            return;
        };
        self.install_developed_thumbnail_at(index, thumbnail, context, revision);
    }

    #[cfg(target_os = "android")]
    pub(crate) fn install_android_developed_thumbnail(
        &mut self,
        raw_uri: &str,
        thumbnail: RawThumbnail,
        context: &egui::Context,
        revision: u64,
    ) {
        let asset_id = LibraryAssetId::Android(raw_uri.to_owned());
        let Some(index) = self.entry_indices.get(&asset_id).copied() else {
            return;
        };
        self.install_developed_thumbnail_at(index, thumbnail, context, revision);
    }

    pub(crate) fn invalidate_adjustment_thumbnail_for_asset(&mut self, asset: &LibraryAsset) {
        if let Some(index) = self.entry_indices.get(&asset.id).copied() {
            self.invalidate_adjustment_thumbnail_at(index);
        }
    }

    pub(super) fn invalidate_adjustment_thumbnail_at(&mut self, index: usize) {
        let Some(entry) = self.entries.get_mut(index) else {
            return;
        };
        entry.texture = None;
        entry.resident_thumbnail = None;
        entry.texture_is_resident = false;
        entry.thumbnail_size = None;
        entry.layout_size = entry.asset.metadata.dimensions_hint;
        entry.thumbnail_error = None;
        entry.thumbnail_failures = 0;
        entry.thumbnail_retry_after = None;
        entry.thumbnail_queued = false;
        entry.developed_thumbnail = false;
        entry.developed_thumbnail_pending = false;
    }

    pub(super) fn install_developed_thumbnail_at(
        &mut self,
        index: usize,
        thumbnail: RawThumbnail,
        context: &egui::Context,
        revision: u64,
    ) {
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [thumbnail.width as usize, thumbnail.height as usize],
            &thumbnail.rgba,
        );
        self.entries[index].texture = Some(context.load_texture(
            format!("library-developed-thumbnail-{index}-{revision}"),
            image,
            egui::TextureOptions::LINEAR,
        ));
        let decoded_size = [thumbnail.width, thumbnail.height];
        let resident_thumbnail = make_resident_thumbnail(&thumbnail);
        self.entries[index].thumbnail_size = Some(decoded_size);
        self.entries[index].layout_size = Some(decoded_size);
        self.entries[index].resident_thumbnail = Some(resident_thumbnail);
        self.entries[index].texture_is_resident = false;
        self.entries[index].thumbnail_error = None;
        self.entries[index].thumbnail_failures = 0;
        self.entries[index].thumbnail_retry_after = None;
        self.entries[index].thumbnail_queued = false;
        self.entries[index].developed_thumbnail = true;
        self.entries[index].developed_thumbnail_pending = false;
    }

    pub(crate) fn evict_old_textures(&mut self, protected_indices: &HashSet<usize>) {
        let limit = if cfg!(target_os = "android") {
            ANDROID_TEXTURE_CACHE_LIMIT
        } else {
            DESKTOP_TEXTURE_CACHE_LIMIT
        };
        self.evict_textures_to_limit_protecting(limit, protected_indices);
        let resident_limit = if cfg!(target_os = "android") {
            ANDROID_RESIDENT_THUMBNAIL_CACHE_LIMIT
        } else {
            DESKTOP_RESIDENT_THUMBNAIL_CACHE_LIMIT
        };
        self.evict_resident_thumbnails_to_limit_protecting(resident_limit, protected_indices);
    }

    #[cfg(target_os = "android")]
    pub(super) fn evict_textures_to_limit(&mut self, limit: usize) {
        self.evict_textures_to_limit_protecting(limit, &HashSet::new());
    }

    pub(super) fn evict_textures_to_limit_protecting(
        &mut self,
        limit: usize,
        protected_indices: &HashSet<usize>,
    ) {
        let texture_count = self
            .entries
            .iter()
            .filter(|entry| entry.texture.is_some())
            .count();
        if texture_count <= limit {
            return;
        }
        let protected_texture_count = protected_indices
            .iter()
            .filter(|&&index| {
                self.entries
                    .get(index)
                    .is_some_and(|entry| entry.texture.is_some())
            })
            .count();
        let effective_limit = limit.max(protected_texture_count);
        if texture_count <= effective_limit {
            return;
        }

        let mut candidates = self
            .entries
            .iter()
            .enumerate()
            .filter(|(index, entry)| entry.texture.is_some() && !protected_indices.contains(index))
            .map(|(index, entry)| (entry.last_used, index))
            .collect::<Vec<_>>();
        candidates.sort_unstable();
        for (_, index) in candidates.into_iter().take(texture_count - effective_limit) {
            self.entries[index].texture = None;
            self.entries[index].texture_is_resident = false;
        }
    }

    pub(super) fn evict_resident_thumbnails_to_limit_protecting(
        &mut self,
        limit: usize,
        protected_indices: &HashSet<usize>,
    ) {
        let resident_count = self
            .entries
            .iter()
            .filter(|entry| entry.resident_thumbnail.is_some())
            .count();
        if resident_count <= limit {
            return;
        }

        let protected_resident_count = protected_indices
            .iter()
            .filter(|&&index| {
                self.entries
                    .get(index)
                    .is_some_and(|entry| entry.resident_thumbnail.is_some())
            })
            .count();
        let effective_limit = limit.max(protected_resident_count);
        if resident_count <= effective_limit {
            return;
        }

        let mut candidates = self
            .entries
            .iter()
            .enumerate()
            .filter(|(index, entry)| {
                entry.resident_thumbnail.is_some() && !protected_indices.contains(index)
            })
            .map(|(index, entry)| (entry.last_used, index))
            .collect::<Vec<_>>();
        candidates.sort_unstable();
        for (_, index) in candidates
            .into_iter()
            .take(resident_count - effective_limit)
        {
            self.entries[index].resident_thumbnail = None;
        }
    }
}

pub(super) fn new_library_entry(asset: LibraryAsset) -> LibraryEntry {
    let layout_size = Some(asset.metadata.dimensions_hint.unwrap_or([3, 2]));
    let review = asset.metadata.review;
    LibraryEntry {
        review,
        asset,
        texture: None,
        resident_thumbnail: None,
        texture_is_resident: false,
        thumbnail_size: None,
        layout_size,
        thumbnail_error: None,
        thumbnail_failures: 0,
        thumbnail_retry_after: None,
        thumbnail_queued: false,
        developed_thumbnail: false,
        developed_thumbnail_pending: false,
        last_used: 0,
    }
}

pub(super) fn same_library_asset_identity(left: &LibraryAsset, right: &LibraryAsset) -> bool {
    left.id == right.id
        && left.metadata.bytes == right.metadata.bytes
        && left.metadata.modified_seconds == right.metadata.modified_seconds
}

pub(super) fn compare_library_entries(
    left: &LibraryEntry,
    right: &LibraryEntry,
    sort_order: LibrarySortOrder,
) -> CmpOrdering {
    let name_order = compare_library_names(&left.asset, &right.asset);

    match sort_order {
        LibrarySortOrder::RatingHighestFirst => right
            .review
            .rating
            .cmp(&left.review.rating)
            .then(name_order),
        LibrarySortOrder::RatingLowestFirst => left
            .review
            .rating
            .cmp(&right.review.rating)
            .then(name_order),
        LibrarySortOrder::FlagPickedFirst => right
            .review
            .flag
            .cmp(&left.review.flag)
            .then_with(|| right.review.rating.cmp(&left.review.rating))
            .then(name_order),
        LibrarySortOrder::FlagRejectedFirst => left
            .review
            .flag
            .cmp(&right.review.flag)
            .then_with(|| right.review.rating.cmp(&left.review.rating))
            .then(name_order),
        LibrarySortOrder::NewestFirst => right
            .asset
            .metadata
            .modified_seconds
            .cmp(&left.asset.metadata.modified_seconds)
            .then(name_order),
        LibrarySortOrder::OldestFirst => left
            .asset
            .metadata
            .modified_seconds
            .cmp(&right.asset.metadata.modified_seconds)
            .then(name_order),
        LibrarySortOrder::NameAscending => name_order,
        LibrarySortOrder::NameDescending => name_order.reverse(),
        LibrarySortOrder::LargestFirst => right
            .asset
            .metadata
            .bytes
            .cmp(&left.asset.metadata.bytes)
            .then(name_order),
        LibrarySortOrder::SmallestFirst => left
            .asset
            .metadata
            .bytes
            .cmp(&right.asset.metadata.bytes)
            .then(name_order),
    }
}

pub(super) fn compare_library_names(left: &LibraryAsset, right: &LibraryAsset) -> CmpOrdering {
    left.display_name
        .to_lowercase()
        .cmp(&right.display_name.to_lowercase())
        .then_with(|| left.display_path.cmp(&right.display_path))
        .then_with(|| left.id.cmp(&right.id))
}

pub(super) fn make_resident_thumbnail(thumbnail: &RawThumbnail) -> RawThumbnail {
    if thumbnail.width <= RESIDENT_THUMBNAIL_EDGE && thumbnail.height <= RESIDENT_THUMBNAIL_EDGE {
        return thumbnail.clone();
    }

    let Some(image) =
        image::RgbaImage::from_raw(thumbnail.width, thumbnail.height, thumbnail.rgba.clone())
    else {
        return thumbnail.clone();
    };
    let image = image::DynamicImage::ImageRgba8(image)
        .thumbnail(RESIDENT_THUMBNAIL_EDGE, RESIDENT_THUMBNAIL_EDGE)
        .to_rgba8();
    let (width, height) = image.dimensions();
    RawThumbnail {
        width,
        height,
        rgba: image.into_raw(),
    }
}

pub(super) fn loaded_library_thumbnail(
    thumbnail: RawThumbnail,
    developed: bool,
) -> LoadedLibraryThumbnail {
    let resident_thumbnail = make_resident_thumbnail(&thumbnail);
    LoadedLibraryThumbnail {
        thumbnail,
        resident_thumbnail,
        review: None,
        developed,
        developed_thumbnail_stale: false,
        developed_render_pending: false,
    }
}

#[cfg(not(target_os = "android"))]
pub(super) fn loaded_library_raw_preview_pending_development(
    thumbnail: RawThumbnail,
) -> LoadedLibraryThumbnail {
    let mut loaded = loaded_library_thumbnail(thumbnail, false);
    loaded.developed_thumbnail_stale = true;
    loaded.developed_render_pending = true;
    loaded
}

pub(super) fn loaded_library_raw_preview_with_stale_edits(
    thumbnail: RawThumbnail,
) -> LoadedLibraryThumbnail {
    let mut loaded = loaded_library_thumbnail(thumbnail, false);
    loaded.developed_thumbnail_stale = true;
    loaded
}
