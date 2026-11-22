//! Reusing the full-size coverage buffers of mask rasterization.
//!
//! A fresh allocation of several megabytes is mapped page by page as it is
//! first written. On a phone that costs more than drawing a radial or linear
//! shape into it: a 1296x1296 coverage took about 4 ms to allocate and fill
//! and 0.45 ms to fill again (Pixel 7 Pro), and an export rasterizes every
//! mask once per tile. So each thread keeps the largest buffer it finished
//! with, up to [`MAX_KEPT_BYTES`], and the next raster starts from it. A
//! buffer handed out holds exactly what a new one would, so the coverage
//! drawn into it is the same.

use rayon::prelude::*;
use std::cell::RefCell;

/// The largest buffer a thread keeps: a 2048x2048 coverage, the mask atlas of
/// a computer's preview. Larger ones, such as a computer's export tiles, are
/// freed as before.
const MAX_KEPT_BYTES: usize = 2048 * 2048 * std::mem::size_of::<f32>();

thread_local! {
    static KEPT: RefCell<Vec<f32>> = const { RefCell::new(Vec::new()) };
}

/// A coverage buffer of `len` values, each `value`: the kept one when it is
/// large enough, otherwise a new one.
pub(super) fn coverage_buffer(len: usize, value: f32) -> Vec<f32> {
    let kept = KEPT.with(|kept| {
        let mut kept = kept.borrow_mut();
        if kept.capacity() >= len {
            std::mem::take(&mut *kept)
        } else {
            Vec::new()
        }
    });
    if kept.capacity() < len {
        return vec![value; len];
    }
    let mut buffer = kept;
    buffer.truncate(len);
    buffer.par_iter_mut().for_each(|slot| *slot = value);
    buffer.resize(len, value);
    buffer
}

/// Keeps `buffer` for this thread's next raster if it is larger than the one
/// kept and within [`MAX_KEPT_BYTES`].
pub(super) fn recycle(buffer: Vec<f32>) {
    if buffer.capacity() * std::mem::size_of::<f32>() > MAX_KEPT_BYTES {
        return;
    }
    KEPT.with(|kept| {
        let mut kept = kept.borrow_mut();
        if buffer.capacity() > kept.capacity() {
            *kept = buffer;
        }
    });
}
