//! The delay and hand-over of a quick preview while the pointer rests on a
//! choice in a list: a preset, a LUT or a camera profile. Each list owns one
//! [`HoverPreview`] keyed by what its rows choose.
use std::time::{Duration, Instant};

/// How long the pointer must rest on a choice before the preview renders.
/// Without a delay, moving across a list re-renders the photo once per row.
pub(crate) const HOVER_PREVIEW_DELAY: Duration = Duration::from_millis(150);

pub(crate) struct HoverPreview<K> {
    /// How long the pointer must rest before the preview renders.
    delay: Duration,
    /// The choice under the pointer this frame. Written by the view and
    /// consumed every frame, so a hidden list ends the preview.
    requested: Option<K>,
    /// The choice under the pointer and when the pointer reached it.
    pending: Option<(K, Instant)>,
    /// The choice rendered on the preview.
    shown: Option<K>,
    /// A choice that was just applied. It is not previewed again until the
    /// pointer leaves it.
    suppressed: Option<K>,
}

impl<K> Default for HoverPreview<K> {
    fn default() -> Self {
        Self::with_delay(HOVER_PREVIEW_DELAY)
    }
}

impl<K> HoverPreview<K> {
    /// A preview that renders after the pointer rests for `delay`, for
    /// choices whose preview costs more than a usual one.
    pub(crate) fn with_delay(delay: Duration) -> Self {
        Self {
            delay,
            requested: None,
            pending: None,
            shown: None,
            suppressed: None,
        }
    }

    /// Reports the choice under the pointer for this frame.
    pub(crate) fn request(&mut self, key: K) {
        self.requested = Some(key);
    }

    /// The choice rendered on the preview.
    pub(crate) fn shown(&self) -> Option<&K> {
        self.shown.as_ref()
    }
}

impl<K: Clone + PartialEq> HoverPreview<K> {
    /// Ends the preview of `key` because it was just applied. Returns whether
    /// a preview was on screen.
    pub(crate) fn suppress<Q: ToOwned<Owned = K> + ?Sized>(&mut self, key: &Q) -> bool {
        self.suppressed = Some(key.to_owned());
        self.pending = None;
        self.shown.take().is_some()
    }

    /// Advances the hover delay. Returns whether the shown choice changed,
    /// and how long to wait before the pending choice is due.
    pub(crate) fn advance(&mut self, now: Instant) -> (bool, Option<Duration>) {
        let requested = self.requested.take();
        if self.suppressed.is_some() && requested != self.suppressed {
            self.suppressed = None;
        }
        let requested = requested.filter(|key| Some(key) != self.suppressed.as_ref());

        let mut wait = None;
        let next = match requested {
            None => {
                self.pending = None;
                None
            }
            Some(key) => {
                let (pending_key, since) = match &self.pending {
                    Some((pending_key, since)) if *pending_key == key => {
                        (pending_key.clone(), *since)
                    }
                    _ => {
                        self.pending = Some((key.clone(), now));
                        (key, now)
                    }
                };
                let waited = now.saturating_duration_since(since);
                if waited >= self.delay {
                    Some(pending_key)
                } else {
                    wait = Some(self.delay - waited);
                    // Keep the current preview until the next one is due, so
                    // moving between rows does not flash the unedited photo.
                    self.shown.clone()
                }
            }
        };
        let changed = next != self.shown;
        self.shown = next;
        (changed, wait)
    }
}
