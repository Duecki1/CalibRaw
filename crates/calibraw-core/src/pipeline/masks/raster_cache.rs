//! Reusable mask sampling and grow/feather preparation. No lens inversions,
//! pixel scans or distance transforms run
//! with a cache mutex held, and callers retain prepared data through eviction.

use super::{chamfer_distance, LensGeometryMap, MaskImage};
use std::collections::{hash_map::DefaultHasher, VecDeque};
use std::hash::{Hash, Hasher};
use std::mem::size_of;
use std::sync::{Arc, Mutex, OnceLock, Weak};

const MAX_ENTRIES: usize = 32;
const CORRECTED_UV_MAX_ENTRIES: usize = 8;
const CONTOUR_BYTES: usize = if cfg!(target_os = "android") {
    24 * 1024 * 1024
} else {
    96 * 1024 * 1024
};
const SOURCE_BYTES: usize = if cfg!(target_os = "android") {
    24 * 1024 * 1024
} else {
    96 * 1024 * 1024
};

struct CacheEntry<T> {
    data: Arc<T>,
    bytes: usize,
}

struct CacheState<T> {
    entries: VecDeque<CacheEntry<T>>,
    bytes: usize,
    // Insertion/pruning changes membership. Touching an entry only changes LRU
    // order, so it need not invalidate equality checks performed outside locks.
    revision: u64,
}

struct BoundedCache<T> {
    state: Mutex<CacheState<T>>,
    max_bytes: usize,
    max_entries: usize,
}

impl<T> BoundedCache<T> {
    fn new(max_bytes: usize, max_entries: usize) -> Self {
        Self {
            state: Mutex::new(CacheState {
                entries: VecDeque::with_capacity(max_entries),
                bytes: 0,
                revision: 0,
            }),
            max_bytes,
            max_entries,
        }
    }

    fn candidates(&self, matches_key: impl Fn(&T) -> bool) -> (u64, Vec<Arc<T>>) {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let candidates = state
            .entries
            .iter()
            .rev()
            .filter(|entry| matches_key(&entry.data))
            .map(|entry| Arc::clone(&entry.data))
            .collect();
        (state.revision, candidates)
    }

    fn touch(&self, data: &Arc<T>) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(index) = state
            .entries
            .iter()
            .position(|entry| Arc::ptr_eq(&entry.data, data))
        {
            let entry = state.entries.remove(index).unwrap();
            state.entries.push_back(entry);
        }
    }

    fn find(
        &self,
        matches_key: impl Fn(&T) -> bool,
        matches_value: impl Fn(&T) -> bool,
    ) -> Option<Arc<T>> {
        let (_, candidates) = self.candidates(matches_key);
        let found = candidates.into_iter().find(|data| matches_value(data))?;
        self.touch(&found);
        Some(found)
    }

    fn insert(
        &self,
        prepared: Arc<T>,
        bytes: usize,
        matches_key: impl Fn(&T) -> bool,
        matches_value: impl Fn(&T) -> bool,
    ) -> Arc<T> {
        // Oversized preparations remain usable by this caller without evicting
        // the smaller reusable entries or exceeding the retained-byte budget.
        if bytes > self.max_bytes || self.max_entries == 0 {
            return prepared;
        }
        loop {
            let (revision, candidates) = self.candidates(&matches_key);
            if let Some(found) = candidates.into_iter().find(|data| matches_value(data)) {
                self.touch(&found);
                return found;
            }
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if state.revision != revision {
                continue;
            }
            let mut evicted = Vec::new();
            while state.entries.len() >= self.max_entries || state.bytes > self.max_bytes - bytes {
                let entry = state.entries.pop_front().unwrap();
                state.bytes -= entry.bytes;
                evicted.push(entry);
            }
            state.bytes += bytes;
            state.entries.push_back(CacheEntry {
                data: Arc::clone(&prepared),
                bytes,
            });
            state.revision = state.revision.wrapping_add(1);
            drop(state);
            // Free potentially large allocations after releasing the mutex.
            drop(evicted);
            return prepared;
        }
    }

    fn remove_if(&self, should_remove: impl Fn(&T) -> bool) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let mut removed = Vec::new();
        for index in (0..state.entries.len()).rev() {
            if should_remove(&state.entries[index].data) {
                let entry = state.entries.remove(index).unwrap();
                state.bytes -= entry.bytes;
                removed.push(entry);
            }
        }
        if !removed.is_empty() {
            state.revision = state.revision.wrapping_add(1);
        }
        drop(state);
        drop(removed);
    }
}

struct CorrectedUvEntry {
    extent: [u32; 2],
    region: [u32; 4],
    full_size: [u32; 2],
    // Retain the mapping allocation so its address cannot be reused for a new
    // lens while this entry remains cached.
    lens: LensGeometryMap,
    uv: Arc<Vec<[f32; 2]>>,
}

impl CorrectedUvEntry {
    fn byte_len(&self) -> usize {
        size_of::<Self>() + size_of::<Vec<[f32; 2]>>() + self.uv.capacity() * size_of::<[f32; 2]>()
    }
}

struct CorrectedUvCache(BoundedCache<CorrectedUvEntry>);

impl CorrectedUvCache {
    fn get(
        &self,
        extent: [u32; 2],
        region: [u32; 4],
        full_size: [u32; 2],
        lens: &LensGeometryMap,
        prepare: impl FnOnce() -> Vec<[f32; 2]>,
    ) -> Arc<Vec<[f32; 2]>> {
        let matches_key = |entry: &CorrectedUvEntry| {
            entry.extent == extent
                && entry.region == region
                && entry.full_size == full_size
                && entry.lens.shares_mapping(lens)
        };
        if let Some(found) = self.0.find(matches_key, |_| true) {
            return Arc::clone(&found.uv);
        }
        // Inversion can be expensive; prepare only after the lookup releases
        // the mutex, then let insertion reuse a concurrent preparation.
        let prepared = Arc::new(CorrectedUvEntry {
            extent,
            region,
            full_size,
            lens: lens.clone(),
            uv: Arc::new(prepare()),
        });
        let bytes = prepared.byte_len();
        Arc::clone(&self.0.insert(prepared, bytes, matches_key, |_| true).uv)
    }
}

pub(super) fn corrected_uv(
    extent: [u32; 2],
    region: [u32; 4],
    full_size: [u32; 2],
    lens: &LensGeometryMap,
    prepare: impl FnOnce() -> Vec<[f32; 2]>,
) -> Arc<Vec<[f32; 2]>> {
    static CACHE: OnceLock<CorrectedUvCache> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            CorrectedUvCache(BoundedCache::new(CONTOUR_BYTES, CORRECTED_UV_MAX_ENTRIES))
        })
        .get(extent, region, full_size, lens, prepare)
}

pub(super) struct SourceFrontier {
    width: u32,
    height: u32,
    pixels: Weak<[u8]>,
    pixel_bytes: usize,
    pairs: Vec<(u32, u32)>,
}

impl SourceFrontier {
    fn prepare(source: &MaskImage) -> Self {
        let width = source.width as usize;
        let height = source.height as usize;
        let mut horizontal_runs = vec![0u32; source.pixels.len()];
        for y in 0..height {
            let mut x = 0;
            while x < width {
                let start = x;
                while x < width && source.pixels[y * width + x] >= 128 {
                    x += 1;
                }
                let length = (x - start) as u32;
                horizontal_runs[y * width + start..y * width + x].fill(length);
                x += usize::from(length == 0);
            }
        }

        // Every selected vertical segment has one vertical length. For that
        // segment, max(min(h * sx, v * sy)) only needs its largest horizontal
        // run. Collapse equal vertical lengths before computing the frontier;
        // even a checkerboard needs only O(height) temporary pairs.
        let mut horizontal_by_vertical = vec![0u32; height + 1];
        for x in 0..width {
            let mut y = 0;
            while y < height {
                let start = y;
                let mut horizontal = 0;
                while y < height && source.pixels[y * width + x] >= 128 {
                    horizontal = horizontal.max(horizontal_runs[y * width + x]);
                    y += 1;
                }
                let vertical = y - start;
                horizontal_by_vertical[vertical] = horizontal_by_vertical[vertical].max(horizontal);
                y += usize::from(vertical == 0);
            }
        }
        let mut pairs = Vec::new();
        let mut largest_horizontal = 0;
        for vertical in (1..=height).rev() {
            let horizontal = horizontal_by_vertical[vertical];
            if horizontal > largest_horizontal {
                pairs.push((horizontal, vertical as u32));
                largest_horizontal = horizontal;
            }
        }
        Self {
            width: source.width,
            height: source.height,
            pixels: Arc::downgrade(&source.pixels),
            pixel_bytes: source.pixels.len(),
            pairs,
        }
    }

    pub(super) fn core_radius(&self, scale_x: f32, scale_y: f32) -> Option<f32> {
        let thickest = self
            .pairs
            .iter()
            .map(|&(horizontal, vertical)| {
                (horizontal as f32 * scale_x).min(vertical as f32 * scale_y)
            })
            .fold(0.0f32, f32::max);
        (thickest > 0.0).then_some(thickest * 0.45)
    }

    fn byte_len(&self) -> usize {
        // A weak Arc keeps its allocation reserved after the last image drops.
        // Account for that storage too, until the next lookup prunes the entry.
        size_of::<Self>() + self.pairs.capacity() * size_of::<(u32, u32)>() + self.pixel_bytes
    }
}

struct SourceCache(BoundedCache<SourceFrontier>);

impl SourceCache {
    fn get(&self, source: &MaskImage) -> Arc<SourceFrontier> {
        self.0.remove_if(|entry| entry.pixels.strong_count() == 0);
        let pixels = Arc::downgrade(&source.pixels);
        let matches_key = |entry: &SourceFrontier| {
            entry.width == source.width
                && entry.height == source.height
                && Weak::ptr_eq(&entry.pixels, &pixels)
        };
        if let Some(found) = self.0.find(matches_key, |_| true) {
            return found;
        }
        let prepared = Arc::new(SourceFrontier::prepare(source));
        let bytes = prepared.byte_len();
        self.0.insert(prepared, bytes, matches_key, |_| true)
    }
}

pub(super) fn source_frontier(source: &MaskImage) -> Arc<SourceFrontier> {
    static CACHE: OnceLock<SourceCache> = OnceLock::new();
    CACHE
        .get_or_init(|| SourceCache(BoundedCache::new(SOURCE_BYTES, MAX_ENTRIES)))
        .get(source)
}

pub(super) struct PreparedContour {
    width: usize,
    height: usize,
    fingerprint: u64,
    binary: Vec<u8>,
    pub(super) signed_distance: Vec<f32>,
    pub(super) deepest_inside: f32,
}

impl PreparedContour {
    fn prepare(binary: Vec<u8>, width: usize, height: usize, fingerprint: u64) -> Self {
        // Cached contours must match an uncached grow/feather bit-for-bit, so keep
        // the same two distance passes and subtraction order.
        let distance_to_inside = chamfer_distance(&binary, width, height, 1);
        let mut signed_distance = chamfer_distance(&binary, width, height, 0);
        let deepest_inside = signed_distance
            .iter()
            .zip(&binary)
            .filter(|(_, inside)| **inside == 1)
            .map(|(distance, _)| *distance)
            .fold(0.0f32, f32::max);
        for (distance, inside) in signed_distance.iter_mut().zip(distance_to_inside) {
            *distance -= inside;
        }
        Self {
            width,
            height,
            fingerprint,
            binary,
            signed_distance,
            deepest_inside,
        }
    }

    fn byte_len(&self) -> usize {
        size_of::<Self>()
            + self.binary.capacity()
            + self.signed_distance.capacity() * size_of::<f32>()
    }
}

struct ContourCache(BoundedCache<PreparedContour>);

impl ContourCache {
    fn get(&self, binary: Vec<u8>, width: usize, height: usize) -> Option<Arc<PreparedContour>> {
        let first = *binary.first()?;
        if binary.iter().all(|value| *value == first) {
            return None;
        }
        let mut hasher = DefaultHasher::new();
        binary.hash(&mut hasher);
        Some(self.get_with_fingerprint(binary, width, height, hasher.finish()))
    }

    fn get_with_fingerprint(
        &self,
        binary: Vec<u8>,
        width: usize,
        height: usize,
        fingerprint: u64,
    ) -> Arc<PreparedContour> {
        let matches_key = |entry: &PreparedContour| {
            entry.width == width && entry.height == height && entry.fingerprint == fingerprint
        };
        if let Some(found) = self.0.find(matches_key, |entry| entry.binary == binary) {
            return found;
        }
        let prepared = Arc::new(PreparedContour::prepare(binary, width, height, fingerprint));
        let bytes = prepared.byte_len();
        let candidate = Arc::clone(&prepared);
        self.0.insert(prepared, bytes, matches_key, |entry| {
            entry.binary == candidate.binary
        })
    }
}

pub(super) fn prepared_contour(
    binary: Vec<u8>,
    width: usize,
    height: usize,
) -> Option<Arc<PreparedContour>> {
    static CACHE: OnceLock<ContourCache> = OnceLock::new();
    CACHE
        .get_or_init(|| ContourCache(BoundedCache::new(CONTOUR_BYTES, MAX_ENTRIES)))
        .get(binary, width, height)
}

#[cfg(test)]
mod tests;
