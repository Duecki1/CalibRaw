//! Scanning the library folder and applying catalog and thumbnail events.

use super::*;

impl LibraryState {
    pub(crate) fn refresh(&mut self, context: &egui::Context) {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let cancellation = Arc::clone(&self.generation);
        let decoding_paused = Arc::clone(&self.decoding_paused);
        let decode_gate = Arc::clone(&self.decode_gate);
        let thumbnail_workers = self.active_thumbnail_worker_count();
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

        #[cfg(not(target_os = "android"))]
        let worker = {
            let Some(folder) = self.folder.clone() else {
                self.event_receiver = None;
                self.request_sender = None;
                self.scanning = false;
                self.status = "Open a folder to build your photo library.".to_owned();
                return;
            };
            let Some(root_folder) = self.root_folder.clone() else {
                self.event_receiver = None;
                self.request_sender = None;
                self.scanning = false;
                self.status = "Open a top-level folder to build your photo library.".to_owned();
                return;
            };
            self.status = format!("Scanning {}…", folder.display());
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
                    let Some((assets, warning_count, truncated)) = scan else {
                        return;
                    };
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
                            )
                        }),
                    );
                })
        };

        #[cfg(target_os = "android")]
        let worker = {
            let android_app = self.platform.app.clone();
            self.status = "Refreshing CalibRaw library…".to_owned();
            std::thread::Builder::new()
                .name("calibraw-library".to_owned())
                .spawn(move || {
                    let folders = match calibraw_ffi::list_library_folders(&android_app) {
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
                    let documents = match calibraw_ffi::list_library_documents(&android_app) {
                        Ok(documents) => documents,
                        Err(error) => {
                            send_scan_failure(&event_sender, generation, error, &repaint);
                            return;
                        }
                    };
                    let truncated = documents.len() > MAX_LIBRARY_FILES;
                    let mut assets = documents
                        .into_iter()
                        .take(MAX_LIBRARY_FILES)
                        .map(LibraryAsset::from_android_document)
                        .collect::<Vec<_>>();
                    for asset in &mut assets {
                        if let Some(uri) = asset.android_uri() {
                            asset.metadata.dimensions_hint =
                                calibraw_ffi::load_library_display_dimensions(&android_app, uri)
                                    .ok();
                        }
                    }
                    let thumbnail_app = android_app.clone();
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
                        thumbnail_workers,
                        Arc::new(move |asset, stage| {
                            load_android_library_thumbnail(
                                &thumbnail_app,
                                asset,
                                stage,
                                render_edited_thumbnails_during_indexing,
                            )
                        }),
                    );
                })
        };

        if let Err(error) = worker {
            self.event_receiver = None;
            self.request_sender = None;
            self.scanning = false;
            self.catalog_ready = true;
            self.status = format!("Could not start the library scanner: {error}");
        }
    }

    #[cfg(not(target_os = "android"))]
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
                self.status = "The dropped RAW import stopped unexpectedly.".to_owned();
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }
    }

    pub(crate) fn poll(&mut self, context: &egui::Context) {
        let pasted = self
            .asset_transfer_receiver
            .as_ref()
            .map(mpsc::Receiver::try_recv);
        match pasted {
            Some(Ok(completion)) => {
                self.asset_transfer_receiver = None;
                #[cfg(not(target_os = "android"))]
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
                self.status = "The Library asset transfer stopped unexpectedly.".to_owned();
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }

        #[cfg(not(target_os = "android"))]
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
                            Ok(_) => format!(
                                "Folder operation completed in the previous library root {}.",
                                completion.root.display()
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
                    self.status = "The folder operation stopped unexpectedly.".to_owned();
                }
                Some(Err(mpsc::TryRecvError::Empty)) | None => {}
            }
        }

        let mut review_sort_changed = false;
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
                        self.status = "The library scanner stopped unexpectedly.".to_owned();
                    }
                    break;
                }
            };

            match event {
                #[cfg(not(target_os = "android"))]
                ScanEvent::FolderTree { generation, tree }
                    if generation == self.generation.load(Ordering::Acquire) =>
                {
                    self.folder_tree = Some(tree);
                }
                #[cfg(target_os = "android")]
                ScanEvent::AndroidFolders {
                    generation,
                    folders,
                } if generation == self.generation.load(Ordering::Acquire) => {
                    self.platform.folders = folders;
                    let folder_paths = self
                        .platform
                        .folders
                        .iter()
                        .map(|folder| folder.path.as_str())
                        .collect::<HashSet<_>>();
                    self.platform
                        .expanded_folders
                        .retain(|path| path.is_empty() || folder_paths.contains(path.as_str()));
                    let selected_folder = self.platform.folder.clone();
                    self.platform
                        .expanded_folders
                        .extend(android_folder_ancestors(&selected_folder));
                }
                ScanEvent::Catalog {
                    generation,
                    assets,
                    warning_count,
                    truncated,
                } if generation == self.generation.load(Ordering::Acquire) => {
                    let mut previous = std::mem::take(&mut self.entries)
                        .into_iter()
                        .map(|entry| (entry.asset.id.clone(), entry))
                        .collect::<HashMap<_, _>>();
                    self.entries = assets
                        .into_iter()
                        .map(|asset| {
                            if let Some(mut entry) = previous.remove(&asset.id) {
                                if same_library_asset_identity(&entry.asset, &asset) {
                                    entry.review = asset.metadata.review;
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
                    self.selected_assets
                        .retain(|asset_id| self.entry_indices.contains_key(asset_id));
                    if self
                        .selection_anchor
                        .as_ref()
                        .is_some_and(|asset_id| !self.entry_indices.contains_key(asset_id))
                    {
                        self.selection_anchor = None;
                    }
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
                            } = loaded;
                            let decoded_size = [thumbnail.width, thumbnail.height];
                            let install_pixels =
                                display_priority || self.entries[index].texture.is_some();
                            self.entries[index].resident_thumbnail = Some(resident_thumbnail);
                            if let Some(review) = review {
                                review_sort_changed |= self.entries[index].review != review
                                    && matches!(
                                        self.sort_order,
                                        LibrarySortOrder::RatingHighestFirst
                                            | LibrarySortOrder::RatingLowestFirst
                                            | LibrarySortOrder::FlagPickedFirst
                                            | LibrarySortOrder::FlagRejectedFirst
                                    );
                                self.entries[index].review = review;
                                self.entries[index].asset.metadata.review = review;
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
                    self.status = error;
                    break;
                }
                _ => {}
            }
        }
        if review_sort_changed {
            self.sort_entries();
        }
    }
}
