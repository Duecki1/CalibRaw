//! The luminance-range mask on the shared feathered-range track, over a
//! black-to-white backdrop. Solid handles sit at `low` and `high`, where
//! selection is full; hollow handles where each edge's own fade begins.

use super::feathered_range::{self, round_to, FeatheredRange, RangeEdit, RangeHandle, RangePoints};
use crate::pipeline::{luminance_range_weight, LUMINANCE_FEATHER_MAX, LUMINANCE_FEATHER_WIDTH};
use eframe::egui::{self, Color32, Ui};

/// Stored precision of dragged values.
const DECIMALS: i32 = 3;
const DEFAULT: LuminanceRange = LuminanceRange {
    low: 0.2,
    high: 0.8,
    low_feather: 0.15,
    high_feather: 0.15,
};

/// Linear-luminance bounds and the feather (0–1) of the edge below `low` and
/// above `high`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LuminanceRange {
    pub(crate) low: f32,
    pub(crate) high: f32,
    pub(crate) low_feather: f32,
    pub(crate) high_feather: f32,
}

impl FeatheredRange for LuminanceRange {
    /// A fade may run past black or white; it is drawn at the track end.
    fn points(&self) -> RangePoints {
        let ramp =
            |feather: f32| feather.clamp(0.0, LUMINANCE_FEATHER_MAX) * LUMINANCE_FEATHER_WIDTH;
        RangePoints {
            fade_in: self.low - ramp(self.low_feather),
            full_from: self.low,
            full_to: self.high,
            fade_out: self.high + ramp(self.high_feather),
        }
    }

    fn set_points(&mut self, edit: RangeEdit) {
        let round = |value| round_to(value, DECIMALS);
        if let Some(edit) = edit.start {
            let (low, fade) = edit.resolve(self.low, round);
            self.low = low;
            if let Some(fade) = fade {
                self.low_feather = (low - fade) / LUMINANCE_FEATHER_WIDTH;
            }
        }
        if let Some(edit) = edit.end {
            let (high, fade) = edit.resolve(self.high, round);
            self.high = high;
            if let Some(fade) = fade {
                self.high_feather = (fade - high) / LUMINANCE_FEATHER_WIDTH;
            }
        }
    }

    fn handle_text(&self, handle: RangeHandle) -> String {
        match handle {
            RangeHandle::Start => format!("Low {:.2}", self.low),
            RangeHandle::End => format!("High {:.2}", self.high),
            RangeHandle::StartFeather => format!("Low feather {:.2}", self.low_feather),
            RangeHandle::EndFeather => format!("High feather {:.2}", self.high_feather),
        }
    }

    fn weight(&self, t: f32) -> f32 {
        luminance_range_weight(
            t,
            self.low,
            self.high,
            [self.low_feather, self.high_feather],
        )
    }

    fn reset(&mut self) {
        *self = DEFAULT;
    }
}

/// Shows the luminance range and returns whether it changed.
pub(crate) fn luminance_range_slider(ui: &mut Ui, range: &mut LuminanceRange) -> bool {
    let before = *range;
    ui.label("Luminance range");
    feathered_range::feathered_range_track(
        ui,
        range,
        "Luminance range",
        "Shadows are on the left. Solid handles set the darkest and brightest fully selected tones; hollow handles set where each edge's fade begins.",
        |painter, track| {
            const COLUMNS: usize = 64;
            let mut mesh = egui::Mesh::default();
            for column in 0..=COLUMNS {
                let t = column as f32 / COLUMNS as f32;
                // The track is linear luminance; show each value as it displays.
                let gray = Color32::from_gray(egui::ecolor::gamma_u8_from_linear_f32(t));
                let x = egui::lerp(track.x_range(), t);
                mesh.colored_vertex(egui::pos2(x, track.top()), gray);
                mesh.colored_vertex(egui::pos2(x, track.bottom()), gray);
            }
            for column in 0..COLUMNS as u32 {
                let i = column * 2;
                mesh.add_triangle(i, i + 1, i + 2);
                mesh.add_triangle(i + 1, i + 3, i + 2);
            }
            painter.add(egui::Shape::mesh(mesh));
        },
    );
    *range != before
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::components::feathered_range::{drag_range, HandleDrag};

    #[test]
    fn handles_sit_at_the_bounds_and_where_each_fade_begins() {
        let range = LuminanceRange {
            low: 0.3,
            high: 0.7,
            low_feather: 0.2,
            high_feather: 0.4,
        };
        let points = range.points();
        assert_eq!((points.full_from, points.full_to), (0.3, 0.7));
        let low_foot = 0.3 - 0.2 * LUMINANCE_FEATHER_WIDTH;
        let high_foot = 0.7 + 0.4 * LUMINANCE_FEATHER_WIDTH;
        assert!((points.fade_in - low_foot).abs() < 1e-6);
        assert!((points.fade_out - high_foot).abs() < 1e-6);
        assert_eq!(range.weight(0.5), 1.0);
        assert_eq!(range.weight(low_foot), 0.0);
        assert_eq!(range.weight(high_foot), 0.0);
    }

    #[test]
    fn each_hollow_handle_sets_only_its_own_feather() {
        let start = DEFAULT;
        let mut range = start;
        drag_range(&mut range, &start, RangeHandle::EndFeather, 0.035);
        assert!((range.high_feather - 0.25).abs() < 2e-3, "{range:?}");
        assert_eq!(
            (range.low, range.high, range.low_feather),
            (start.low, start.high, start.low_feather)
        );
    }

    #[test]
    fn solid_handles_keep_their_feather_until_the_fade_meets_black_then_squeeze_it() {
        let ramp = DEFAULT.low_feather * LUMINANCE_FEATHER_WIDTH;
        let mut range = DEFAULT;
        let mut drag = HandleDrag::new(&range, RangeHandle::Start);
        // While the fade fits, it is carried unchanged.
        drag.move_to(&mut range, ramp);
        assert!((range.low - ramp).abs() < 1e-3, "{range:?}");
        assert_eq!(range.low_feather, DEFAULT.low_feather);
        // Past that, the fade stays at black and the feather narrows.
        drag.move_to(&mut range, 0.02);
        assert_eq!(range.low, 0.02);
        assert!(range.points().fade_in.abs() < 1e-6, "{range:?}");
        assert!(range.low_feather < DEFAULT.low_feather, "{range:?}");
        // The bound reaches black.
        drag.move_to(&mut range, 0.0);
        assert_eq!((range.low, range.low_feather), (0.0, 0.0));
        // Back again, the narrowed feather is carried unchanged.
        drag.move_to(&mut range, 0.3);
        assert_eq!((range.low, range.low_feather), (0.3, 0.0));
        // The other edge was never touched.
        assert_eq!(
            (range.high, range.high_feather),
            (DEFAULT.high, DEFAULT.high_feather)
        );
        // A fade can open all the way to black.
        let mut wide = DEFAULT;
        drag_range(&mut wide, &DEFAULT, RangeHandle::StartFeather, -1.0);
        assert!(wide.points().fade_in.abs() < 1e-6, "{wide:?}");
        assert_eq!(wide.low, DEFAULT.low);
    }

    #[test]
    fn a_fade_already_past_black_stays_draggable() {
        // Older settings: the dark fade starts below black, at -0.11.
        let start = LuminanceRange {
            low: 0.1,
            low_feather: 0.6,
            ..DEFAULT
        };
        assert!(start.points().fade_in < 0.0);
        // Moving right carries it unchanged.
        let mut range = start;
        drag_range(&mut range, &start, RangeHandle::Start, 0.05);
        assert!((range.low - 0.15).abs() < 1e-6, "{range:?}");
        assert_eq!(range.low_feather, start.low_feather);
        // Moving left keeps the fade where it is rather than carrying it
        // further off, and the bound still reaches black.
        let mut range = start;
        drag_range(&mut range, &start, RangeHandle::Start, -0.05);
        assert!((range.low - 0.05).abs() < 1e-6, "{range:?}");
        assert!((range.points().fade_in - start.points().fade_in).abs() < 1e-3);
        drag_range(&mut range, &start, RangeHandle::Start, -1.0);
        assert_eq!(range.low, 0.0);
        assert!(range.low_feather <= start.low_feather, "{range:?}");
    }

    #[test]
    fn crossing_handles_push_the_other_edge_with_its_feather() {
        let mut range = DEFAULT;
        let mut drag = HandleDrag::new(&range, RangeHandle::Start);
        // The pushed edge keeps its feather while its fade fits.
        drag.move_to(&mut range, 0.9);
        assert!((range.high - 0.9).abs() < 1e-6, "{range:?}");
        assert_eq!(
            (range.low_feather, range.high_feather),
            (DEFAULT.low_feather, DEFAULT.high_feather)
        );
        // Both handles reach white; only the pushed fade is squeezed.
        drag.move_to(&mut range, 1.0);
        assert_eq!((range.low, range.high), (1.0, 1.0));
        assert_eq!(range.low_feather, DEFAULT.low_feather);
        assert_eq!(range.high_feather, 0.0);
    }

    #[test]
    fn pushed_handles_stay_where_they_were_left() {
        let mut range = DEFAULT;
        let mut drag = HandleDrag::new(&range, RangeHandle::Start);
        // Drag the low bound past the high one, then back.
        for target in [0.5, 0.9] {
            drag.move_to(&mut range, target);
        }
        assert!((range.high - 0.9).abs() < 1e-6, "{range:?}");
        for target in [0.6, 0.3] {
            drag.move_to(&mut range, target);
        }
        assert!((range.low - 0.3).abs() < 1e-6, "{range:?}");
        assert!((range.high - 0.9).abs() < 1e-6, "{range:?}");
    }
}
