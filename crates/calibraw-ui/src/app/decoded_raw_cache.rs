//! Decoded RAWs kept for reopening, shared by document loads and neighbour
//! prefetch.
//!
//! An entry is one file decoded with one camera-profile request, together with
//! what opening it derives from the decode alone: its Lensfun catalog and its
//! most recent lens-corrected copy. The RAW a document renders from also keeps
//! its full-resolution highlight analysis, so a revisit skips that work too.
//!
//! Decodes are single-flight: asking for an entry that another thread is
//! decoding waits for that decode instead of starting a second one. The limit
//! counts files, least recently used first out; entries still decoding are
//! never evicted.

use crate::pipeline::{lensfun_catalog, CameraProfileMode, LensfunCatalog, LensfunLens, LoadedRaw};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};

/// Everything that determines a decode's result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DecodeKey {
    /// The file's identity, including its size and modification time, so an
    /// edited file is decoded again.
    pub(crate) source: String,
    pub(crate) profile_mode: CameraProfileMode,
    pub(crate) profile_folder: Option<PathBuf>,
    pub(crate) requested_profile: Option<PathBuf>,
}

/// A decoded RAW and what is derived from it alone.
pub(crate) struct DecodedRaw {
    original: Arc<LoadedRaw>,
    lens_catalog: OnceLock<LensfunCatalog>,
    /// Held while a correction runs, so a second request for the same
    /// selection waits for it instead of correcting again.
    lens_corrected: Mutex<Option<(LensfunLens, Arc<LoadedRaw>)>>,
}

impl DecodedRaw {
    fn new(original: LoadedRaw) -> Self {
        Self {
            original: Arc::new(original),
            lens_catalog: OnceLock::new(),
            lens_corrected: Mutex::new(None),
        }
    }

    pub(crate) fn original(&self) -> &Arc<LoadedRaw> {
        &self.original
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
        selection: &LensfunLens,
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

    #[cfg(any(not(target_os = "android"), test))]
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

    /// The entry for `key`, decoded by `decode` unless it is cached or another
    /// thread is already decoding it, in which case this waits for that decode.
    /// A failed decode is not kept; a caller waiting for it decodes itself.
    pub(crate) fn get_or_decode(
        &self,
        key: &DecodeKey,
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
        let decoded = Arc::new(DecodedRaw::new(decode()?));
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

    fn key(source: &str) -> DecodeKey {
        DecodeKey {
            source: source.to_owned(),
            profile_mode: CameraProfileMode::Automatic,
            profile_folder: None,
            requested_profile: None,
        }
    }

    fn raw(width: u32) -> LoadedRaw {
        LoadedRaw::from_scene_linear_rec2020(width, 1, vec![0.5; width as usize * 3]).unwrap()
    }

    fn cached_sources(cache: &DecodedRawCache) -> Vec<String> {
        cache
            .lock()
            .entries
            .iter()
            .map(|entry| entry.key.source.clone())
            .collect()
    }

    #[test]
    fn a_cached_decode_is_reused_and_kept_most_recent() {
        let cache = DecodedRawCache::new(2);
        let decodes = AtomicUsize::new(0);
        let decode = |width| {
            decodes.fetch_add(1, Ordering::Relaxed);
            Ok(raw(width))
        };
        let first = cache.get_or_decode(&key("a"), || decode(1)).unwrap();
        cache.get_or_decode(&key("b"), || decode(2)).unwrap();
        let again = cache.get_or_decode(&key("a"), || decode(3)).unwrap();
        assert!(Arc::ptr_eq(&first, &again));
        assert_eq!(decodes.load(Ordering::Relaxed), 2);
        // "b" is now least recently used and makes room for "c".
        cache.get_or_decode(&key("c"), || decode(4)).unwrap();
        assert_eq!(cached_sources(&cache), ["a", "c"]);
    }

    #[test]
    fn different_profile_requests_are_different_entries() {
        let cache = DecodedRawCache::new(4);
        let automatic = key("a");
        let selected = DecodeKey {
            requested_profile: Some(PathBuf::from("profiles/neutral.dcp")),
            ..key("a")
        };
        let first = cache.get_or_decode(&automatic, || Ok(raw(1))).unwrap();
        let second = cache.get_or_decode(&selected, || Ok(raw(2))).unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
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
                        .get_or_decode(&key("a"), || {
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

    #[test]
    fn failed_and_panicked_decodes_leave_no_entry() {
        let cache = DecodedRawCache::new(2);
        assert!(cache
            .get_or_decode(&key("a"), || Err(anyhow::anyhow!("unreadable")))
            .is_err());
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = cache.get_or_decode(&key("b"), || panic!("decoder bug"));
        }));
        assert!(panicked.is_err());
        assert!(cached_sources(&cache).is_empty());
        // Both can be decoded afterwards.
        cache.get_or_decode(&key("a"), || Ok(raw(1))).unwrap();
        cache.get_or_decode(&key("b"), || Ok(raw(1))).unwrap();
    }

    #[test]
    fn clearing_during_a_decode_does_not_keep_its_result() {
        let cache = DecodedRawCache::new(2);
        cache
            .get_or_decode(&key("a"), || {
                cache.clear();
                Ok(raw(1))
            })
            .unwrap();
        assert!(cached_sources(&cache).is_empty());
    }

    #[test]
    fn a_zero_limit_decodes_without_keeping() {
        let cache = DecodedRawCache::new(0);
        cache.get_or_decode(&key("a"), || Ok(raw(1))).unwrap();
        assert!(cached_sources(&cache).is_empty());
        let cache = DecodedRawCache::new(3);
        for source in ["a", "b", "c"] {
            cache.get_or_decode(&key(source), || Ok(raw(1))).unwrap();
        }
        cache.set_limit(1);
        assert_eq!(cached_sources(&cache), ["c"]);
    }

    #[test]
    fn lens_corrections_are_reused_per_selection() {
        let decoded = DecodedRaw::new(raw(4));
        let corrections = AtomicUsize::new(0);
        let correct = |raw: &LoadedRaw| {
            corrections.fetch_add(1, Ordering::Relaxed);
            Ok(raw.clone())
        };
        let lens = |model: &str| LensfunLens {
            maker: "Tamron".to_owned(),
            model: model.to_owned(),
            ..LensfunLens::default()
        };
        let first = decoded.lens_corrected(&lens("28-75"), correct).unwrap();
        let again = decoded.lens_corrected(&lens("28-75"), correct).unwrap();
        assert!(Arc::ptr_eq(&first, &again));
        let other = decoded.lens_corrected(&lens("17-28"), correct).unwrap();
        assert_eq!(corrections.load(Ordering::Relaxed), 2);
        // A failed correction keeps the previous one.
        assert!(decoded
            .lens_corrected(&lens("70-180"), |_| Err(anyhow::anyhow!("no profile")))
            .is_err());
        let kept = decoded.lens_corrected(&lens("17-28"), correct).unwrap();
        assert!(Arc::ptr_eq(&other, &kept));
        assert_eq!(corrections.load(Ordering::Relaxed), 2);
    }
}
