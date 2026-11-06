//! Scanning the library folder and applying catalog and thumbnail events.

use super::*;
use crate::i18n::{tr, tr_fmt};

impl LibraryState {
    /// Scans the shown folder again; a linked folder's tree is walked again too.
    pub(crate) fn refresh(&mut self, context: &egui::Context) {
        if !self.media.root.is_empty() {
            self.media.tree_walk_due = true;
        }
        self.rescan(context);
    }

    /// Scans the shown folder again and keeps a linked folder's tree, which
    /// selecting another of its folders does not change.
    pub(in crate::ui::library) fn rescan(&mut self, context: &egui::Context) {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let cancellation = Arc::clone(&self.generation);
        let decoding_paused = Arc::clone(&self.decoding_paused);
        let decode_gate = Arc::clone(&self.decode_gate);
        // Also the limit on photos read at once in a media-library scan.
        let thumbnail_workers = self
            .active_thumbnail_worker_count()
            .clamp(1, maximum_thumbnail_worker_count());
        let render_edited_thumbnails_during_indexing =
            self.render_edited_thumbnails_during_indexing;
        let repaint = context.clone();
        let (event_sender, event_receiver) = mpsc::sync_channel(MAX_PENDING_THUMBNAIL_RESULTS);
        let (request_sender, request_receiver) = mpsc::sync_channel(MAX_PENDING_THUMBNAILS);
        self.event_receiver = Some(event_receiver);
        self.request_sender = Some(request_sender);
        self.thumbnail_progress = ThumbnailBackgroundProgress::default();
        for entry in &mut self.entries {
            entry.thumbnail_queued = false;
            entry.thumbnail_error = None;
            entry.thumbnail_failures = 0;
            entry.thumbnail_retry_after = None;
            if !render_edited_thumbnails_during_indexing {
                entry.developed_thumbnail = false;
                entry.developed_thumbnail_pending = false;
            }
        }
        self.scanning = true;
        self.catalog_ready = !self.entries.is_empty();
        self.usage_clock = 0;

        let worker = if self.platform.caps().has_folders() {
            let Some(folder) = self.folder.clone() else {
                self.event_receiver = None;
                self.request_sender = None;
                self.scanning = false;
                self.status = tr("Open a folder to build your photo library.").to_owned();
                return;
            };
            let Some(root_folder) = self.root_folder.clone() else {
                self.event_receiver = None;
                self.request_sender = None;
                self.scanning = false;
                self.status = tr("Open a top-level folder to build your photo library.").to_owned();
                return;
            };
            self.status = tr_fmt("Scanning {folder}…", &[("folder", &folder.display())]);
            std::thread::Builder::new()
                .name("calibraw-library".to_owned())
                .spawn(move || {
                    let tree_sender = event_sender.clone();
                    let tree_repaint = repaint.clone();
                    let tree_cancellation = Arc::clone(&cancellation);
                    if let Err(error) = std::thread::Builder::new()
                        .name("calibraw-library-folders".to_owned())
                        .spawn(move || {
                            if let Some(tree) = scan_folder_tree(&root_folder, || {
                                tree_cancellation.load(Ordering::Acquire) != generation
                            }) {
                                let _ =
                                    tree_sender.send(ScanEvent::FolderTree { generation, tree });
                                tree_repaint.request_repaint();
                            }
                        })
                    {
                        log::warn!("could not start the library folder scanner: {error}");
                    }
                    let scan = match scan_folder(&folder, || {
                        cancellation.load(Ordering::Acquire) != generation
                    }) {
                        Ok(result) => result,
                        Err(error) => {
                            send_scan_failure(&event_sender, generation, error, &repaint);
                            return;
                        }
                    };
                    let Some(FolderScan {
                        assets,
                        warning_count,
                        truncated,
                        deferred_metadata,
                    }) = scan
                    else {
                        return;
                    };
                    let deferred_metadata = Arc::new(deferred_metadata);
                    let worker_metadata = Arc::clone(&deferred_metadata);
                    run_thumbnail_workers(
                        ThumbnailWorker {
                            assets,
                            warning_count,
                            truncated,
                            generation,
                            cancellation,
                            decoding_paused,
                            decode_gate,
                            event_sender,
                            request_receiver,
                            repaint,
                        },
                        thumbnail_workers,
                        Arc::new(move |asset, stage| {
                            load_desktop_library_thumbnail(
                                asset,
                                stage,
                                render_edited_thumbnails_during_indexing,
                                &worker_metadata,
                            )
                        }),
                    );
                    // Keeps metadata read since the last periodic save when the
                    // workers stop before every photo was read.
                    deferred_metadata.save();
                })
        } else {
            // A linked folder may be slow to walk, so its photos do not wait
            // for its folders.
            self.start_linked_tree_walk(context);
            let platform = Arc::clone(&self.platform);
            let linked_folder = self.media.root.clone();
            let folder = self.media.folder.clone();
            self.status = tr("Refreshing CalibRaw library…").to_owned();
            std::thread::Builder::new()
                .name("calibraw-library".to_owned())
                .spawn(move || {
                    if linked_folder.is_empty() {
                        let folders = match platform.list_library_folders() {
                            Ok(folders) => folders,
                            Err(error) => {
                                send_scan_failure(&event_sender, generation, error, &repaint);
                                return;
                            }
                        };
                        if event_sender
                            .send(ScanEvent::AndroidFolders {
                                generation,
                                folders,
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                    let sidecar_writes = platform.sidecar_write_count();
                    let documents = match platform.list_library_documents() {
                        Ok(documents) => documents,
                        Err(error) => {
                            send_scan_failure(&event_sender, generation, error, &repaint);
                            return;
                        }
                    };
                    let MediaFolderScan {
                        assets,
                        truncated,
                        deferred_metadata,
                    } = catalog_media_folder(
                        &*platform,
                        &linked_folder,
                        &folder,
                        documents,
                        sidecar_writes,
                    );
                    let deferred_metadata = Arc::new(deferred_metadata);
                    let worker_metadata = Arc::clone(&deferred_metadata);
                    let extraction = ExtractionPermits::new(thumbnail_workers);
                    run_thumbnail_workers(
                        ThumbnailWorker {
                            assets,
                            warning_count: 0,
                            truncated,
                            generation,
                            cancellation,
                            decoding_paused,
                            decode_gate,
                            event_sender,
                            request_receiver,
                            repaint,
                        },
                        MEDIA_LIBRARY_PREVIEW_WORKERS.max(thumbnail_workers),
                        Arc::new(move |asset, _stage| {
                            load_media_library_thumbnail(
                                &*platform,
                                &worker_metadata,
                                &extraction,
                                asset,
                            )
                        }),
                    );
                    // Keeps what was read since the last periodic save when the
                    // workers stop before every photo was read.
                    deferred_metadata.save();
                })
        };

        if let Err(error) = worker {
            self.event_receiver = None;
            self.request_sender = None;
            self.scanning = false;
            self.catalog_ready = true;
            self.status = tr_fmt(
                "Could not start the library scanner: {error}",
                &[("error", &error)],
            );
        }
    }

    /// Walks the shown linked folder's tree when a refresh asked for it and no
    /// walk runs. A failure is only logged: the photo scan reaches the same
    /// folder and reports it.
    fn start_linked_tree_walk(&mut self, context: &egui::Context) {
        if self.media.root.is_empty() || !self.media.tree_walk_due || self.media.tree_walk.is_some()
        {
            return;
        }
        self.media.tree_walk_due = false;
        let root = self.media.root.clone();
        let platform = Arc::clone(&self.platform);
        let repaint = context.clone();
        let (sender, result) = mpsc::channel();
        let walked_root = root.clone();
        let walk = std::thread::Builder::new()
            .name("calibraw-library-folders".to_owned())
            .spawn(move || {
                // The library may have stopped waiting for this walk.
                let _ = sender.send(platform.list_linked_folder_tree(&walked_root));
                repaint.request_repaint();
            });
        match walk {
            Ok(_) => self.media.tree_walk = Some(LinkedTreeWalk { root, result }),
            Err(error) => log::warn!("could not walk the linked folder: {error}"),
        }
    }

    fn poll_linked_tree_walk(&mut self, context: &egui::Context) {
        let Some(walk) = self.media.tree_walk.as_ref() else {
            return;
        };
        let result = match walk.result.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("the linked folder walk stopped unexpectedly".to_owned())
            }
        };
        let shown = walk.root == self.media.root;
        self.media.tree_walk = None;
        match result {
            Ok(folders) if shown => self.show_android_folders(folders),
            Ok(_) => {}
            Err(error) => log::warn!("{error}"),
        }
        // A refresh while it ran, such as a folder created meanwhile.
        self.start_linked_tree_walk(context);
    }

    /// Shows the folders of the media library or of a linked folder, keeping
    /// the selected folder's ancestors open.
    fn show_android_folders(&mut self, folders: Vec<crate::platform::LibraryFolder>) {
        self.media.folders = folders;
        let folder_paths = self
            .media
            .folders
            .iter()
            .map(|folder| folder.path.as_str())
            .collect::<HashSet<_>>();
        self.media
            .expanded_folders
            .retain(|path| path.is_empty() || folder_paths.contains(path.as_str()));
        let selected_folder = self.media.folder.clone();
        self.media
            .expanded_folders
            .extend(android_folder_ancestors(&selected_folder));
    }

    pub(crate) fn poll_dropped_raw_import(&mut self, context: &egui::Context) {
        let imported = self
            .raw_import_receiver
            .as_ref()
            .map(mpsc::Receiver::try_recv);
        match imported {
            Some(Ok(result)) => {
                self.raw_import_receiver = None;
                if !result.imported.is_empty() || !result.imported_folders.is_empty() {
                    self.refresh(context);
                }
                self.status = raw_import_status(&result);
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.raw_import_receiver = None;
                self.status = tr("The dropped RAW import stopped unexpectedly.").to_owned();
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }
    }

    pub(crate) fn poll(&mut self, context: &egui::Context) {
        self.poll_linked_tree_walk(context);
        let pasted = self
            .asset_transfer_receiver
            .as_ref()
            .map(mpsc::Receiver::try_recv);
        match pasted {
            Some(Ok(completion)) => {
                self.asset_transfer_receiver = None;
                if completion.clear_clipboard {
                    self.image_clipboard = None;
                } else if let Some(remaining) = completion.remaining_clipboard {
                    self.image_clipboard = Some(remaining);
                }
                self.clear_selection();
                self.status = completion.result.unwrap_or_else(|error| error);
                self.refresh(context);
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.asset_transfer_receiver = None;
                self.status = tr("The Library asset transfer stopped unexpectedly.").to_owned();
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }

        // Folder imports and operations; never started for a media library.
        {
            self.poll_dropped_raw_import(context);

            let folder_completed = self
                .folder_operation_receiver
                .as_ref()
                .map(mpsc::Receiver::try_recv);
            match folder_completed {
                Some(Ok(completion)) => {
                    self.folder_operation_receiver = None;
                    if self.root_folder.as_ref() != Some(&completion.root) {
                        self.status = match completion.result {
                            Ok(_) => tr_fmt(
                                "Folder operation completed in the previous library root {root}.",
                                &[("root", &completion.root.display())],
                            ),
                            Err(error) => error,
                        };
                    } else {
                        match completion.result {
                            Ok(result) => self.apply_folder_operation_result(result, context),
                            Err(error) => {
                                self.refresh(context);
                                self.status = error;
                            }
                        }
                    }
                }
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    self.folder_operation_receiver = None;
                    self.status = tr("The folder operation stopped unexpectedly.").to_owned();
                }
                Some(Err(mpsc::TryRecvError::Empty)) | None => {}
            }
        }

        let mut review_sort_changed = false;
        let mut date_sort_changed = false;
        for _ in 0..MAX_EVENTS_PER_FRAME {
            let received = self.event_receiver.as_ref().map(mpsc::Receiver::try_recv);
            let event = match received {
                Some(Ok(event)) => event,
                Some(Err(mpsc::TryRecvError::Empty)) | None => break,
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    self.event_receiver = None;
                    self.request_sender = None;
                    self.scanning = false;
                    if !self.catalog_ready {
                        self.status = tr("The library scanner stopped unexpectedly.").to_owned();
                    }
                    break;
                }
            };

            match event {
                ScanEvent::FolderTree { generation, tree }
                    if generation == self.generation.load(Ordering::Acquire) =>
                {
                    self.folder_tree = Some(tree);
                }
                ScanEvent::AndroidFolders {
                    generation,
                    folders,
                } if generation == self.generation.load(Ordering::Acquire) => {
                    self.show_android_folders(folders);
                }
                ScanEvent::Catalog {
                    generation,
                    assets,
                    warning_count,
                    truncated,
                } if generation == self.generation.load(Ordering::Acquire) => {
                    self.media
                        .unavailable_roots
                        .remove(self.media.root.as_str());
                    let mut previous = std::mem::take(&mut self.entries)
                        .into_iter()
                        .map(|entry| (entry.asset.id.clone(), entry))
                        .collect::<HashMap<_, _>>();
                    self.entries = assets
                        .into_iter()
                        .map(|mut asset| {
                            if let Some(mut entry) = previous.remove(&asset.id) {
                                if same_library_asset_identity(&entry.asset, &asset) {
                                    // A media-library scan reads no sidecars, so the review on
                                    // screen stays: it may hold a rating not saved yet, and a
                                    // thumbnail already on screen is not loaded again to
                                    // supply one.
                                    if asset.android_uri().is_some() {
                                        asset.metadata.review = entry.asset.metadata.review;
                                    }
                                    entry.asset = asset;
                                    entry.thumbnail_error = None;
                                    entry.thumbnail_queued = false;
                                    entry.developed_thumbnail_pending = false;
                                    entry.last_used = 0;
                                    return entry;
                                }
                            }
                            new_library_entry(asset)
                        })
                        .collect();
                    self.sort_entries();
                    self.forget_missing_selection();
                    self.scanning = false;
                    self.catalog_ready = true;
                    self.thumbnail_progress
                        .begin(generation, self.entries.len());
                    self.status = catalog_status(warning_count, truncated);
                }
                ScanEvent::Thumbnail {
                    generation,
                    asset_id,
                    display_priority,
                    final_thumbnail,
                    result,
                } if generation == self.generation.load(Ordering::Acquire) => {
                    if final_thumbnail {
                        self.thumbnail_progress
                            .record_completion(generation, asset_id.clone());
                    }
                    let Some(index) = self.entry_indices.get(&asset_id).copied() else {
                        continue;
                    };
                    self.entries[index].thumbnail_queued = false;
                    match result {
                        Ok(loaded) => {
                            if let Some(metadata) = loaded.display_metadata {
                                date_sort_changed |= apply_display_metadata(
                                    &mut self.entries[index].asset.metadata,
                                    metadata,
                                );
                            }
                            if self.entries[index].developed_thumbnail
                                && !loaded.developed
                                && !loaded.developed_render_pending
                            {
                                continue;
                            }
                            let LoadedLibraryThumbnail {
                                thumbnail,
                                resident_thumbnail,
                                review,
                                developed,
                                developed_thumbnail_stale,
                                developed_render_pending,
                                ..
                            } = loaded;
                            let decoded_size = [thumbnail.width, thumbnail.height];
                            let install_pixels =
                                display_priority || self.entries[index].texture.is_some();
                            self.entries[index].resident_thumbnail = Some(resident_thumbnail);
                            if let Some(review) =
                                review.filter(|_| !self.review_save_pending(&asset_id))
                            {
                                review_sort_changed |= self.show_review(&asset_id, review);
                            }
                            self.entries[index].texture_is_resident = false;
                            if install_pixels {
                                let image = egui::ColorImage::from_rgba_unmultiplied(
                                    [thumbnail.width as usize, thumbnail.height as usize],
                                    &thumbnail.rgba,
                                );
                                self.entries[index].texture = Some(context.load_texture(
                                    format!("library-thumbnail-{generation}-{index}"),
                                    image,
                                    egui::TextureOptions::LINEAR,
                                ));
                            }
                            self.entries[index].thumbnail_size = Some(decoded_size);
                            self.entries[index].layout_size = Some(decoded_size);
                            self.entries[index].thumbnail_error = None;
                            self.entries[index].thumbnail_failures = 0;
                            self.entries[index].thumbnail_retry_after = None;
                            self.entries[index].developed_thumbnail = developed;
                            self.entries[index].thumbnail_queued = developed_render_pending;
                            self.entries[index].developed_thumbnail_pending =
                                developed_thumbnail_stale;
                        }
                        Err(error) => {
                            if !self.entries[index].developed_thumbnail {
                                let entry = &mut self.entries[index];
                                entry.thumbnail_failures =
                                    entry.thumbnail_failures.saturating_add(1);
                                let exponent =
                                    u32::from(entry.thumbnail_failures.saturating_sub(1).min(5));
                                let delay = Duration::from_secs(1_u64 << exponent)
                                    .min(THUMBNAIL_RETRY_MAX_DELAY);
                                entry.thumbnail_error = Some(error);
                                entry.thumbnail_retry_after = Some(Instant::now() + delay);
                                entry.developed_thumbnail_pending = false;
                                context.request_repaint_after(delay);
                            }
                        }
                    }
                }
                ScanEvent::Failed { generation, error }
                    if generation == self.generation.load(Ordering::Acquire) =>
                {
                    self.scanning = false;
                    self.catalog_ready = true;
                    self.event_receiver = None;
                    self.request_sender = None;
                    if let Some(folder) = self.linked_folder_label().map(str::to_owned) {
                        self.media.pending_error = Some(tr_fmt(
                            "{folder} is not available. Reconnect its storage or link it again. ({error})",
                            &[("folder", &folder), ("error", &error)],
                        ));
                        let root = self.media.root.clone();
                        self.media.unavailable_roots.insert(root);
                    }
                    self.status = error;
                    break;
                }
                _ => {}
            }
        }
        if review_sort_changed || date_sort_changed {
            self.sort_entries();
        }
    }
}
