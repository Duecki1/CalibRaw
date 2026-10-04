//! The thumbnail worker pool: prioritized requests, pausing and generation checks.

use super::*;

pub(in crate::ui::library) fn run_thumbnail_workers(
    worker: ThumbnailWorker,
    worker_count: usize,
    load: ThumbnailLoader,
) {
    let ThumbnailWorker {
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
    } = worker;
    if cancellation.load(Ordering::Acquire) != generation {
        return;
    }
    let work_queue = Arc::new(Mutex::new(ThumbnailWorkQueue::new(generation, &assets)));
    if event_sender
        .send(ScanEvent::Catalog {
            generation,
            assets: assets.clone(),
            warning_count,
            truncated,
        })
        .is_err()
    {
        return;
    }
    repaint.request_repaint();

    let assets = Arc::new(assets);
    let request_receiver = Arc::new(Mutex::new(request_receiver));
    let worker_count = worker_count.clamp(1, maximum_thumbnail_worker_count());
    let mut handles = Vec::with_capacity(worker_count);
    for worker_index in 0..worker_count {
        let cancellation = Arc::clone(&cancellation);
        let decoding_paused = Arc::clone(&decoding_paused);
        let decode_gate = Arc::clone(&decode_gate);
        let event_sender = event_sender.clone();
        let request_receiver = Arc::clone(&request_receiver);
        let work_queue = Arc::clone(&work_queue);
        let repaint = repaint.clone();
        let load = Arc::clone(&load);
        let assets = Arc::clone(&assets);
        let spawn = std::thread::Builder::new()
            .name(format!("calibraw-thumbnail-{worker_index}"))
            .spawn(move || {
                run_one_thumbnail_worker(ThumbnailWorkerContext {
                    assets,
                    generation,
                    cancellation,
                    decoding_paused,
                    decode_gate,
                    event_sender,
                    request_receiver,
                    work_queue,
                    repaint,
                    load,
                })
            });
        match spawn {
            Ok(handle) => handles.push(handle),
            Err(error) => log::warn!("could not start thumbnail worker {worker_index}: {error}"),
        }
    }

    if handles.is_empty() {
        send_scan_failure(
            &event_sender,
            generation,
            "Could not start any thumbnail workers.".to_owned(),
            &repaint,
        );
        return;
    }
    for handle in handles {
        if handle.join().is_err() {
            log::warn!("a thumbnail worker panicked");
        }
    }
}

struct ThumbnailWorkerContext {
    assets: Arc<Vec<LibraryAsset>>,
    pub(super) generation: u64,
    pub(super) cancellation: Arc<AtomicU64>,
    decoding_paused: Arc<AtomicBool>,
    decode_gate: Arc<RwLock<()>>,
    event_sender: mpsc::SyncSender<ScanEvent>,
    request_receiver: Arc<Mutex<mpsc::Receiver<ThumbnailRequest>>>,
    work_queue: Arc<Mutex<ThumbnailWorkQueue>>,
    repaint: egui::Context,
    pub(super) load: ThumbnailLoader,
}

fn run_one_thumbnail_worker(context: ThumbnailWorkerContext) {
    let ThumbnailWorkerContext {
        assets,
        generation,
        cancellation,
        decoding_paused,
        decode_gate,
        event_sender,
        request_receiver,
        work_queue,
        repaint,
        load,
    } = context;
    while cancellation.load(Ordering::Acquire) == generation {
        let received = request_receiver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .try_recv();
        let (request, initial_background) = match received {
            Ok(request) => (request, false),
            Err(mpsc::TryRecvError::Empty) => {
                if decoding_paused.load(Ordering::Acquire) {
                    std::thread::sleep(THUMBNAIL_PAUSE_POLL_INTERVAL);
                    continue;
                }
                let background = work_queue
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .background
                    .pop_front();
                let Some(request) = background else {
                    std::thread::sleep(THUMBNAIL_QUEUE_POLL_INTERVAL);
                    continue;
                };
                (request, true)
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                if decoding_paused.load(Ordering::Acquire) {
                    break;
                }
                let background = work_queue
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .background
                    .pop_front();
                let Some(request) = background else {
                    break;
                };
                (request, true)
            }
        };
        if request.generation != generation {
            continue;
        }
        if !work_queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .claim(&request, initial_background)
        {
            continue;
        }
        let result = loop {
            while decoding_paused.load(Ordering::Acquire) && !request.display_priority {
                if cancellation.load(Ordering::Acquire) != generation {
                    return;
                }
                std::thread::sleep(THUMBNAIL_PAUSE_POLL_INTERVAL);
            }

            let decode_guard = decode_gate
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if cancellation.load(Ordering::Acquire) != generation {
                return;
            }
            if decoding_paused.load(Ordering::Acquire) && !request.display_priority {
                drop(decode_guard);
                continue;
            }
            let Some(asset) = assets.iter().find(|asset| asset.id == request.asset_id) else {
                break Err("thumbnail asset disappeared from the catalog".to_owned());
            };
            break load(asset, request.stage);
        };
        let queue_developed_preview = request.stage == ThumbnailLoadStage::RawPreview
            && result
                .as_ref()
                .is_ok_and(|loaded| loaded.developed_render_pending);
        let display_priority = {
            let mut queue = work_queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            queue.finish(&request)
        };
        if event_sender
            .send(ScanEvent::Thumbnail {
                generation,
                asset_id: request.asset_id.clone(),
                display_priority,
                final_thumbnail: !queue_developed_preview,
                result,
            })
            .is_err()
        {
            break;
        }
        if queue_developed_preview {
            work_queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .schedule_developed_preview(generation, request.asset_id.clone());
        }
        repaint.request_repaint();
    }
}
