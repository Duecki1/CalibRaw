//! Decoded RAWs kept for reopening, shared by document loads and neighbour
//! prefetch.
//!
//! An entry is one decoded file, together with what opening it derives from
//! the decode alone: its Lensfun catalog and its most recent lens-corrected
//! copy. The RAW a document renders from also keeps its full-resolution
//! highlight analysis, so a revisit skips that work too.
//!
//! A camera profile changes only how the samples' colour is read, so one
//! entry serves every camera-profile request: the decode itself for the
//! request it was decoded for, and a copy sharing its samples for another
//! (`LoadedRaw::with_camera_profile_selection`). Only a decode that read the
//! RAW's calibration signature can match DCPs; one decoded with the embedded
//! matrix alone serves only requests for the embedded matrix.
//!
//! Decodes are single-flight: asking for an entry that another thread is
//! decoding waits for that decode instead of starting a second one. The limit
//! counts files, least recently used first out; entries still decoding are
//! never evicted.

use crate::pipeline::{
    lensfun_catalog, CameraProfileMode, LensCorrection, LensfunCatalog, LoadedRaw,
};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};

/// The camera-profile choice a document asks a decode for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DecodeRequest {
    pub(crate) mode: CameraProfileMode,
    pub(crate) folder: Option<PathBuf>,
    pub(crate) profile: Option<PathBuf>,
}

impl DecodeRequest {
    /// Whether the decode reads the RAW's calibration signature, which
    /// matching a DCP needs.
    fn matches_dcps(&self) -> bool {
        self.mode != CameraProfileMode::MatrixOnly
    }
}

/// Which decodes an entry holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DecodeKey {
    /// The file's identity, including its size and modification time, so an
    /// edited file is decoded again.
    pub(crate) source: String,
    /// Whether the decode can match DCPs (`DecodeRequest::matches_dcps`).
    pub(crate) matches_dcps: bool,
}

impl DecodeKey {
    pub(crate) fn new(source: String, request: &DecodeRequest) -> Self {
        Self {
            source,
            matches_dcps: request.matches_dcps(),
        }
    }
}

/// A decoded RAW and what is derived from it alone.
pub(crate) struct DecodedRaw {
    /// The camera-profile request `original` was decoded for.
    request: DecodeRequest,
    original: Arc<LoadedRaw>,
    lens_catalog: OnceLock<LensfunCatalog>,
    /// Held while a correction runs, so a second request for the same
    /// selection waits for it instead of correcting again.
    lens_corrected: Mutex<Option<(LensCorrection, Arc<LoadedRaw>)>>,
    /// The decode with the camera profile of the latest other request.
    recolored: Mutex<Option<Recolored>>,
}

/// A decode with the camera profile of another request than its own.
struct Recolored {
    request: DecodeRequest,
    original: Arc<LoadedRaw>,
    /// The latest lens-corrected copy of `original`.
    lens_corrected: Option<(LensCorrection, Arc<LoadedRaw>)>,
}

impl DecodedRaw {
    fn new(request: DecodeRequest, original: LoadedRaw) -> Self {
        Self {
            request,
            original: Arc::new(original),
            lens_catalog: OnceLock::new(),
            lens_corrected: Mutex::new(None),
            recolored: Mutex::new(None),
        }
    }

    /// The RAW with the camera profile `request` chooses, exactly as a decode
    /// for `request` would give it. The entry's cache lookup ensures the
    /// decode can serve `request` (`DecodeKey::matches_dcps`).
    pub(crate) fn original_for(&self, request: &DecodeRequest) -> anyhow::Result<Arc<LoadedRaw>> {
        if *request == self.request {
            return Ok(Arc::clone(&self.original));
        }
        let mut slot = lock(&self.recolored);
        if let Some(recolored) = slot
            .as_ref()
            .filter(|recolored| recolored.request == *request)
        {
            return Ok(Arc::clone(&recolored.original));
        }
        let original = if self.original.color_source.is_none() {
            // Rendered photos have no camera profile, so every request
            // decodes them alike.
            Arc::clone(&self.original)
        } else {
            let recolored = self
                .original
                .with_camera_profile_selection(
                    request.mode,
                    request.folder.as_deref(),
                    request.profile.as_deref(),
                )
                .ok_or_else(|| {
                    anyhow::anyhow!("the RAW was decoded without what this camera profile needs")
                })??;
            Arc::new(recolored)
        };
        *slot = Some(Recolored {
            request: request.clone(),
            original: Arc::clone(&original),
            lens_corrected: None,
        });
        Ok(original)
    }

    /// The lens-corrected RAW for `selection` with the camera profile
    /// `request` chooses. The correction itself is made once for the decode
    /// ([`Self::lens_corrected`]); another profile shares its samples.
    pub(crate) fn lens_corrected_for(
        &self,
        request: &DecodeRequest,
        selection: &LensCorrection,
        correct: impl FnOnce(&LoadedRaw) -> anyhow::Result<LoadedRaw>,
    ) -> anyhow::Result<Arc<LoadedRaw>> {
        let corrected = self.lens_corrected(selection, correct)?;
        if *request == self.request {
            return Ok(corrected);
        }
        let original = self.original_for(request)?;
        if Arc::ptr_eq(&original, &self.original) {
            return Ok(corrected);
        }
        let mut slot = lock(&self.recolored);
        let Some(recolored) = slot
            .as_mut()
            .filter(|recolored| recolored.request == *request)
        else {
            // Another request replaced the copy meanwhile.
            return Ok(Arc::new(corrected.with_profile_color_of(&original)));
        };
        if let Some((cached_selection, corrected)) = &recolored.lens_corrected {
            if cached_selection == selection {
                return Ok(Arc::clone(corrected));
            }
        }
        let recolored_corrected = Arc::new(corrected.with_profile_color_of(&original));
        recolored.lens_corrected = Some((selection.clone(), Arc::clone(&recolored_corrected)));
        Ok(recolored_corrected)
    }

    /// The Lensfun profiles matching this RAW, looked up once.
    pub(crate) fn lens_catalog(&self) -> LensfunCatalog {
        self.lens_catalog
            .get_or_init(|| lensfun_catalog(&self.original))
            .clone()
    }

    /// The RAW corrected for `selection` by `correct`, reusing the last
    /// correction when its selection matches. Failures are not kept.
    pub(crate) fn lens_corrected(
        &self,
        selection: &LensCorrection,
        correct: impl FnOnce(&LoadedRaw) -> anyhow::Result<LoadedRaw>,
    ) -> anyhow::Result<Arc<LoadedRaw>> {
        let mut slot = lock(&self.lens_corrected);
        if let Some((cached_selection, corrected)) = slot.as_ref() {
            if cached_selection == selection {
                return Ok(Arc::clone(corrected));
            }
        }
        let corrected = Arc::new(correct(&self.original)?);
        *slot = Some((selection.clone(), Arc::clone(&corrected)));
        Ok(corrected)
    }
}

/// A handle to the shared cache; clones refer to the same entries.
#[derive(Clone)]
pub(crate) struct DecodedRawCache {
    shared: Arc<Shared>,
}

struct Shared {
    state: Mutex<CacheState>,
    /// Signalled whenever a decode finishes or entries are removed.
    changed: Condvar,
}

struct CacheState {
    limit: usize,
    next_ticket: u64,
    /// Least recently used first.
    entries: VecDeque<Entry>,
}

struct Entry {
    key: DecodeKey,
    slot: Slot,
}

enum Slot {
    /// Being decoded by the holder of this ticket. Tickets keep a decode that
    /// was cleared away from filling a newer entry for the same key.
    Decoding(u64),
    Ready(Arc<DecodedRaw>),
}

impl DecodedRawCache {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(CacheState {
                    limit,
                    next_ticket: 0,
                    entries: VecDeque::new(),
                }),
                changed: Condvar::new(),
            }),
        }
    }

    /// The number of files kept; 0 disables caching.
    pub(crate) fn limit(&self) -> usize {
        self.lock().limit
    }

    pub(crate) fn set_limit(&self, limit: usize) {
        let mut state = self.lock();
        state.limit = limit;
        state.evict_over_limit();
    }

    /// Drops every entry, for settings that change what decodes produce.
    /// Decodes in flight finish for their callers but are not kept.
    pub(crate) fn clear(&self) {
        self.lock().entries.clear();
        self.shared.changed.notify_all();
    }

    /// The entry for `key` if it is decoded already, without waiting for a
    /// decode in progress.
    pub(crate) fn get_ready(&self, key: &DecodeKey) -> Option<Arc<DecodedRaw>> {
        let mut state = self.lock();
        let index = state.entries.iter().position(|entry| entry.key == *key)?;
        let Slot::Ready(decoded) = &state.entries[index].slot else {
            return None;
        };
        let decoded = Arc::clone(decoded);
        state.mark_used(index);
        Some(decoded)
    }

    /// The entry for `key`, decoded for `request` by `decode` unless it is
    /// cached or another thread is already decoding it, in which case this
    /// waits for that decode. A failed decode is not kept; a caller waiting
    /// for it decodes itself.
    pub(crate) fn get_or_decode(
        &self,
        key: &DecodeKey,
        request: &DecodeRequest,
        decode: impl FnOnce() -> anyhow::Result<LoadedRaw>,
    ) -> anyhow::Result<Arc<DecodedRaw>> {
        let mut state = self.lock();
        let ticket = loop {
            match state.entries.iter().position(|entry| entry.key == *key) {
                Some(index) => match &state.entries[index].slot {
                    Slot::Ready(decoded) => {
                        let decoded = Arc::clone(decoded);
                        state.mark_used(index);
                        return Ok(decoded);
                    }
                    Slot::Decoding(_) => {
                        state = self
                            .shared
                            .changed
                            .wait(state)
                            .unwrap_or_else(PoisonError::into_inner);
                    }
                },
                None => break state.begin_decoding(key),
            }
        };
        drop(state);

        let pending = PendingDecode {
            cache: self,
            ticket,
        };
        let decoded = Arc::new(DecodedRaw::new(request.clone(), decode()?));
        pending.finish(Arc::clone(&decoded));
        Ok(decoded)
    }

    fn lock(&self) -> MutexGuard<'_, CacheState> {
        lock(&self.shared.state)
    }
}

impl CacheState {
    /// Reserves an entry for a decode, or returns `None` when caching is off.
    fn begin_decoding(&mut self, key: &DecodeKey) -> Option<u64> {
        if self.limit == 0 {
            return None;
        }
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        self.entries.push_back(Entry {
            key: key.clone(),
            slot: Slot::Decoding(ticket),
        });
        Some(ticket)
    }

    fn position_of_ticket(&self, ticket: u64) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| matches!(entry.slot, Slot::Decoding(held) if held == ticket))
    }

    fn mark_used(&mut self, index: usize) {
        if let Some(entry) = self.entries.remove(index) {
            self.entries.push_back(entry);
        }
    }

    fn evict_over_limit(&mut self) {
        while self.entries.len() > self.limit {
            let Some(index) = self
                .entries
                .iter()
                .position(|entry| matches!(entry.slot, Slot::Ready(_)))
            else {
                break;
            };
            self.entries.remove(index);
        }
    }
}

/// A reserved entry being decoded. Dropping it unfinished (the decode failed
/// or panicked) removes the reservation and wakes waiting callers.
struct PendingDecode<'a> {
    cache: &'a DecodedRawCache,
    ticket: Option<u64>,
}

impl PendingDecode<'_> {
    fn finish(mut self, decoded: Arc<DecodedRaw>) {
        let Some(ticket) = self.ticket.take() else {
            return;
        };
        let mut state = self.cache.lock();
        if let Some(index) = state.position_of_ticket(ticket) {
            state.entries[index].slot = Slot::Ready(decoded);
            state.mark_used(index);
            state.evict_over_limit();
        }
        drop(state);
        self.cache.shared.changed.notify_all();
    }
}

impl Drop for PendingDecode<'_> {
    fn drop(&mut self) {
        let Some(ticket) = self.ticket.take() else {
            return;
        };
        let mut state = self.cache.lock();
        if let Some(index) = state.position_of_ticket(ticket) {
            state.entries.remove(index);
        }
        drop(state);
        self.cache.shared.changed.notify_all();
    }
}

/// Cache state stays consistent across a panic in a decode, which runs
/// without the lock, so a poisoned lock is recovered.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Barrier;

    fn request() -> DecodeRequest {
        DecodeRequest {
            mode: CameraProfileMode::Automatic,
            folder: None,
            profile: None,
        }
    }

    fn key(source: &str) -> DecodeKey {
        DecodeKey::new(source.to_owned(), &request())
    }

    fn raw(width: u32) -> LoadedRaw {
        LoadedRaw::from_scene_linear_rec2020(width, 1, vec![0.5; width as usize * 3]).unwrap()
    }

    #[test]
    fn concurrent_requests_share_one_decode() {
        let cache = DecodedRawCache::new(2);
        let decodes = AtomicUsize::new(0);
        let started = Barrier::new(2);
        let results = std::thread::scope(|scope| {
            let handles = [0, 1].map(|_| {
                scope.spawn(|| {
                    started.wait();
                    cache
                        .get_or_decode(&key("a"), &request(), || {
                            decodes.fetch_add(1, Ordering::Relaxed);
                            std::thread::sleep(std::time::Duration::from_millis(50));
                            Ok(raw(1))
                        })
                        .unwrap()
                })
            });
            handles.map(|handle| handle.join().unwrap())
        });
        assert_eq!(decodes.load(Ordering::Relaxed), 1);
        assert!(Arc::ptr_eq(&results[0], &results[1]));
    }
}
