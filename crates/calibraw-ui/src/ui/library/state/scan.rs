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
        let include_subfolders = self.view.include_subfolders;
        let repaint = context.clone();
        let (event_sender, event_receiver) = mpsc::sync_channel(MAX_PENDING_THUMBNAIL_RESULTS);
        let (request_sender, request_receiver) = mpsc::sync_channel(MAX_PENDING_THUMBNAILS);
        self.event_receiver = Some(event_receiver);
        self.request_sender = Some(request_sender);
        self.thumbnail_progress = ThumbnailBackgroundProgress::default();
        self.developed.cancel();
        let renders_saved_edits = self.renders_saved_edits();
        for entry in &mut self.entries {
            entry.thumbnail_queued = false;
            entry.thumbnail_error = None;
            entry.thumbnail_failures = 0;
            entry.thumbnail_retry_after = None;
            // Every thumbnail loads again, which settles what it shows; a
            // failed render is tried again.
            entry.edits = match std::mem::take(&mut entry.edits) {
                ThumbnailEdits::Rendering | ThumbnailEdits::Stale | ThumbnailEdits::Failed(_)
                    if renders_saved_edits =>
                {
                    ThumbnailEdits::Rendering
                }
                ThumbnailEdits::Rendering | ThumbnailEdits::Stale | ThumbnailEdits::Failed(_) => {
                    ThumbnailEdits::Stale
                }
                // Without rendering, a render on screen keeps out the camera's
                // preview only until the folder is scanned again.
                ThumbnailEdits::Developed if !renders_saved_edits => ThumbnailEdits::Unedited,
                edits => edits,
            };
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
                    let scan = match scan_folder(&folder, include_subfolders, || {
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
                        Arc::new(move |asset| {
                            load_desktop_library_thumbnail(asset, &worker_metadata)
                        }),
                    );
                    // Keeps metadata read since the last periodic save when the
                    // workers stop before every photo was read.
                    deferred_metadata.save();
                })
        } else {
            // A linked folder may be slow to walk, so its photos do not wait
            // for its folders.
            self.start_folder_tree_walk(context);
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
                    let documents = match platform.list_library_documents(include_subfolders) {
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
                        Arc::new(move |asset| {
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

    /// Walks one root's tree when no walk runs: the shown linked folder's
    /// when a refresh asked for it, otherwise that of a root the sidebar shows
    /// without its folders yet. A failure is only logged: the photo scan
    /// reaches the shown folder and reports it, and a root not shown reports
    /// it once opened.
    fn start_folder_tree_walk(&mut self, context: &egui::Context) {
        if self.media.tree_walk.is_some() {
            return;
        }
        let root = if !self.media.root.is_empty() && self.media.tree_walk_due {
            self.media.tree_walk_due = false;
            self.media.root.clone()
        } else {
            let media = &self.media;
            let unlisted = std::iter::once("")
                .chain(
                    media
                        .linked_folders
                        .iter()
                        .map(|folder| folder.key.as_str()),
                )
                .filter(|key| *key != media.root)
                .find(|key| !media.trees.get(*key).is_some_and(|tree| tree.listed));
            match unlisted {
                Some(key) => key.to_owned(),
                None => return,
            }
        };
        let platform = Arc::clone(&self.platform);
        let repaint = context.clone();
        let (sender, result) = mpsc::channel();
        let walked_root = root.clone();
        let walk = std::thread::Builder::new()
            .name("calibraw-library-folders".to_owned())
            .spawn(move || {
                // The library may have stopped waiting for this walk.
                let _ = sender.send(platform.list_folder_tree(&walked_root));
                repaint.request_repaint();
            });
        match walk {
            Ok(_) => self.media.tree_walk = Some(FolderTreeWalk { root, result }),
            Err(error) => {
                log::warn!("could not walk the folder: {error}");
                self.media.trees.entry(root).or_default().listed = true;
            }
        }
    }

    fn poll_folder_tree_walk(&mut self, context: &egui::Context) {
        if let Some(walk) = self.media.tree_walk.as_ref() {
            let result = match walk.result.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Err("the folder walk stopped unexpectedly".to_owned())
                }
            };
            let root = walk.root.clone();
            self.media.tree_walk = None;
            match result {
                // An unlinked folder's walk is dropped.
                Ok(folders) if self.media.has_root(&root) => {
                    self.show_android_folders(&root, folders);
                }
                Ok(_) => {}
                Err(error) => {
                    log::warn!("{error}");
                    if self.media.has_root(&root) {
                        self.media.trees.entry(root).or_default().listed = true;
                    }
                }
            }
        }
        // A refresh while it ran, such as a folder created meanwhile, or a
        // root linked since.
        self.start_folder_tree_walk(context);
    }

    /// Shows the folders of the media library or of a linked folder, keeping
    /// the selected folder's ancestors open in the shown root.
    fn show_android_folders(&mut self, root: &str, folders: Vec<crate::platform::LibraryFolder>) {
        let shown = root == self.media.root;
        let selected_folder = self.media.folder.clone();
        let tree = self.media.trees.entry(root.to_owned()).or_default();
        tree.folders = folders;
        tree.listed = true;
        let folder_paths = tree
            .folders
            .iter()
            .map(|folder| folder.path.as_str())
            .collect::<HashSet<_>>();
        tree.expanded
            .retain(|path| path.is_empty() || folder_paths.contains(path.as_str()));
        if shown {
            tree.expanded
                .extend(android_folder_ancestors(&selected_folder));
        }
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
        self.poll_folder_tree_walk(context);
        self.poll_developed_renders(context);
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
                    let root = self.media.root.clone();
                    self.show_android_folders(&root, folders);
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
                                    // A media-library scan reads no sidecars, so the review and
                                    // edit badge on screen stay: the review may hold a rating not
                                    // saved yet, and a thumbnail already on screen is not loaded
                                    // again to supply either.
                                    if asset.android_uri().is_some() {
                                        asset.metadata.review = entry.asset.metadata.review;
                                        asset.metadata.edited = entry.asset.metadata.edited;
                                    }
                                    entry.asset = asset;
                                    entry.thumbnail_error = None;
                                    entry.thumbnail_queued = false;
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
                    result,
                } if generation == self.generation.load(Ordering::Acquire) => {
                    self.thumbnail_progress
                        .record_completion(generation, asset_id.clone());
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
                            let entry = &self.entries[index];
                            let shown = &entry.edits;
                            let shows_render = *shown == ThumbnailEdits::Developed;
                            let render_kept = shows_render
                                && (entry.texture.is_some() || entry.resident_thumbnail.is_some());
                            // The new state, and whether the tile keeps the
                            // thumbnail it has instead of the loaded one.
                            let (edits, keep_pixels) = match loaded.edits {
                                LoadedEdits::Developed => (ThumbnailEdits::Developed, false),
                                // A load that started before a render was
                                // installed does not replace it.
                                LoadedEdits::Unedited if shows_render => continue,
                                LoadedEdits::Unedited => (ThumbnailEdits::Unedited, false),
                                // A failed render waits for the next scan.
                                LoadedEdits::Stale
                                    if matches!(shown, ThumbnailEdits::Failed(_)) =>
                                {
                                    (shown.clone(), false)
                                }
                                // An earlier render stays until the edits render.
                                LoadedEdits::Stale if self.renders_saved_edits() => {
                                    (ThumbnailEdits::Rendering, render_kept)
                                }
                                LoadedEdits::Stale if shows_render => continue,
                                LoadedEdits::Stale => (ThumbnailEdits::Stale, false),
                            };
                            let LoadedLibraryThumbnail {
                                thumbnail,
                                resident_thumbnail,
                                badges,
                                ..
                            } = loaded;
                            if let Some(badges) = badges {
                                if !self.review_save_pending(&asset_id) {
                                    review_sort_changed |=
                                        self.show_review(&asset_id, badges.review);
                                }
                                self.entries[index].asset.metadata.edited = badges.edited;
                            }
                            let entry = &mut self.entries[index];
                            entry.edits = edits;
                            entry.thumbnail_error = None;
                            entry.thumbnail_failures = 0;
                            entry.thumbnail_retry_after = None;
                            if keep_pixels {
                                continue;
                            }
                            let decoded_size = [thumbnail.width, thumbnail.height];
                            let install_pixels = display_priority || entry.texture.is_some();
                            entry.resident_thumbnail = Some(resident_thumbnail);
                            entry.texture_is_resident = false;
                            if install_pixels {
                                let image = egui::ColorImage::from_rgba_unmultiplied(
                                    [thumbnail.width as usize, thumbnail.height as usize],
                                    &thumbnail.rgba,
                                );
                                entry.texture = Some(context.load_texture(
                                    format!("library-thumbnail-{generation}-{index}"),
                                    image,
                                    egui::TextureOptions::LINEAR,
                                ));
                            }
                            entry.thumbnail_size = Some(decoded_size);
                            entry.layout_size = Some(decoded_size);
                        }
                        Err(error) => {
                            if self.entries[index].edits != ThumbnailEdits::Developed {
                                let entry = &mut self.entries[index];
                                entry.thumbnail_failures =
                                    entry.thumbnail_failures.saturating_add(1);
                                let exponent =
                                    u32::from(entry.thumbnail_failures.saturating_sub(1).min(5));
                                let delay = Duration::from_secs(1_u64 << exponent)
                                    .min(THUMBNAIL_RETRY_MAX_DELAY);
                                entry.thumbnail_error = Some(error);
                                entry.thumbnail_retry_after = Some(Instant::now() + delay);
                                entry.edits = ThumbnailEdits::Unedited;
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
