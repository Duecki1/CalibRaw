//! The export carousel: a stencil over the output, exported as one file per slide.
//!
//! The Export panel's card and the preview return [`CarouselAction`]s, and the preview draws the
//! stencil while that panel is open; the handler here changes the state. The slide count and
//! shape are saved with the settings. The switch and the placement belong to the open photo, so
//! a later normal export never produces slides by surprise. None of this is part of the edit.

use super::*;
use crate::performance_settings::SavedCarousel;
use crate::pipeline::{Carousel, CarouselLayout};

pub(crate) struct CarouselState {
    enabled: bool,
    carousel: Carousel,
    /// While the stencil is dragged: the pointer's offset from its centre, 0–1 across the output.
    grab: Option<[f32; 2]>,
}

/// What the Export panel's card and the preview show.
#[derive(Clone, Copy)]
pub(crate) struct CarouselView {
    pub(crate) enabled: bool,
    pub(crate) carousel: Carousel,
    /// While it is on, where the stencil lands; `None` when these slides cannot be exported.
    pub(crate) layout: Option<CarouselLayout>,
    pub(crate) grab: Option<[f32; 2]>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CarouselAction {
    SetEnabled(bool),
    /// New slide count, shape or size.
    Configure(Carousel),
    /// The stencil was pressed, this far from its centre.
    Grab([f32; 2]),
    /// The dragged stencil's centre moved here, 0–1 across the output.
    MoveTo([f32; 2]),
    Release,
}

impl CarouselState {
    pub(in crate::app) fn new(saved: SavedCarousel) -> Self {
        Self {
            enabled: false,
            carousel: Carousel::new(saved.slides, saved.ratio),
            grab: None,
        }
    }

    pub(in crate::app) fn saved(&self) -> SavedCarousel {
        SavedCarousel {
            slides: self.carousel.slides(),
            ratio: self.carousel.ratio(),
        }
    }

    /// Turns the carousel off and returns the stencil to its largest, centred placement.
    pub(in crate::app) fn reset_for_new_photo(&mut self) {
        self.enabled = false;
        self.carousel = self.carousel.recentered();
        self.grab = None;
    }

    pub(crate) fn grabbed(&self) -> bool {
        self.grab.is_some()
    }

    /// The carousel an export should write, while it is on.
    pub(in crate::app) fn active(&self) -> Option<Carousel> {
        self.enabled.then_some(self.carousel)
    }
}

impl CalibRawApp {
    pub(crate) fn carousel_view(&self) -> CarouselView {
        let state = &self.export.carousel;
        let layout =
            self.source_dimensions()
                .filter(|_| state.enabled)
                .and_then(|(width, height)| {
                    state
                        .carousel
                        .layout(self.develop.geometry, width, height, &self.export.settings)
                        .ok()
                });
        CarouselView {
            enabled: state.enabled,
            carousel: state.carousel,
            layout,
            grab: state.grab,
        }
    }

    pub(crate) fn apply_carousel_action(&mut self, action: CarouselAction) {
        let state = &mut self.export.carousel;
        match action {
            CarouselAction::SetEnabled(enabled) => {
                state.enabled = enabled;
                state.grab = None;
            }
            CarouselAction::Configure(carousel) => {
                let saved_part_changed = carousel.slides() != state.carousel.slides()
                    || carousel.ratio() != state.carousel.ratio();
                state.carousel = carousel;
                if saved_part_changed {
                    self.persist_performance_settings();
                }
            }
            CarouselAction::Grab(grab) => state.grab = Some(grab),
            CarouselAction::MoveTo(center) => state.carousel = state.carousel.with_center(center),
            CarouselAction::Release => state.grab = None,
        }
    }
}
