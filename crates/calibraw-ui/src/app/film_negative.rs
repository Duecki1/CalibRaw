//! Developing the open photo as the positive of a scanned film negative: the
//! handlers of the Film negative card and of the film base picker.

use super::*;
use crate::i18n::tr;
use crate::pipeline::{FilmNegative, FilmSource, FilmStock};

/// Title of the Develop card of a film negative. Its eye shows the scan as it
/// is (`PreviewVisibility`).
pub(crate) const FILM_NEGATIVE_CARD_TITLE: &str = "Film negative";

impl CalibRawApp {
    /// Develops the open photo as a colour negative, estimated as Auto does.
    /// Returns whether it is one now.
    pub(crate) fn enable_film_negative(&mut self) -> bool {
        self.develop.exposure.film.negative().is_some()
            || self.estimate_film_negative(FilmStock::Color, None)
    }

    /// Estimates the negative again: the base from the whole scan, the rest
    /// from what the crop shows now.
    pub(crate) fn auto_film_negative(&mut self) {
        if let Some(negative) = self.develop.exposure.film.negative().copied() {
            self.estimate_film_negative(negative.stock, None);
        }
    }

    /// Develops the negative as `stock`, keeping its base.
    pub(crate) fn set_film_stock(&mut self, stock: FilmStock) {
        if let Some(negative) = self.develop.exposure.film.negative().copied() {
            if negative.stock != stock {
                self.estimate_film_negative(stock, Some(negative.base));
            }
        }
    }

    /// Takes the film base from the area from `area[0]` to `area[1]`
    /// (normalized source coordinates) and estimates the rest anew.
    pub(crate) fn apply_film_base_area(&mut self, area: [[f32; 2]; 2]) {
        self.develop_ui.cancel_area_picker();
        let (Some(negative), Some(raw)) = (
            self.develop.exposure.film.negative().copied(),
            self.develop.loaded_raw.as_deref(),
        ) else {
            return;
        };
        let samples = raw.film_samples_in_area(area[0], area[1], self.develop.exposure.black_point);
        match FilmNegative::base_of_area(&samples) {
            Some(base) => {
                if self.estimate_film_negative(negative.stock, Some(base)) {
                    self.ui.notice =
                        Some(tr("Film base sampled from the selected area.").to_owned());
                }
            }
            None => self.report_error(
                ErrorKind::Sampling,
                tr("Could not read the film there. Choose unexposed film that is not blown out.")
                    .to_owned(),
            ),
        }
    }

    /// Develops the photo as it is again.
    pub(crate) fn remove_film_negative(&mut self) {
        if self.develop_ui.area_picker == Some(AreaPicker::FilmBase) {
            self.develop_ui.cancel_area_picker();
        }
        self.set_film_source(FilmSource::Positive);
    }

    /// Estimates a negative of `stock` with `base`, or one from the whole
    /// scan, and develops the photo as it
    /// (`LoadedRaw::estimate_film_negative`). Reports a failure and returns
    /// false when the photo shows no negative to estimate.
    fn estimate_film_negative(&mut self, stock: FilmStock, base: Option<[f32; 3]>) -> bool {
        let Some(raw) = self.develop.loaded_raw.as_deref() else {
            return false;
        };
        let Some(negative) = raw.estimate_film_negative(
            stock,
            base,
            self.develop.geometry,
            self.develop.exposure.black_point,
        ) else {
            self.report_error(
                ErrorKind::Sampling,
                tr("Could not find a negative to invert. Crop to one frame, or pick its unexposed film.")
                    .to_owned(),
            );
            return false;
        };
        self.set_film_source(FilmSource::Negative(negative));
        true
    }

    fn set_film_source(&mut self, film: FilmSource) {
        if self.develop.exposure.film == film {
            return;
        }
        self.develop.exposure.film = film;
        // Mask sources develop the positive (`ExposureParams::source_reference`),
        // so the masks drawn from them are out of date.
        self.masks.source_cache = None;
        if !self.masks.stack.content_dependencies().is_empty() {
            self.ai.update_needed = true;
        }
        self.mark_pipeline_dirty();
    }
}
