//! The luminance-range mask on the shared feathered-range track, over a
//! black-to-white backdrop. Solid handles sit at `low` and `high`, where
//! selection is full; hollow handles where each edge's own fade begins.

use super::feathered_range::{self, round_to, FeatheredRange, RangeField, RangePoints};
use crate::pipeline::{luminance_range_weight, LUMINANCE_FEATHER_MAX, LUMINANCE_FEATHER_WIDTH};
use eframe::egui::{self, Color32, Ui};

/// Stored precision of dragged values; the fields show two decimals.
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
    /// A fade running past black or white is drawn at the track end.
    fn points(&self) -> RangePoints {
        let ramp =
            |feather: f32| feather.clamp(0.0, LUMINANCE_FEATHER_MAX) * LUMINANCE_FEATHER_WIDTH;
        RangePoints {
            fade_in: (self.low - ramp(self.low_feather)).clamp(0.0, 1.0),
            full_from: self.low.clamp(0.0, 1.0),
            full_to: self.high.clamp(0.0, 1.0),
            fade_out: (self.high + ramp(self.high_feather)).clamp(0.0, 1.0),
        }
    }

    /// Only the edge that moved is rewritten, so the other keeps its exact
    /// values even where its fade runs past black or white.
    fn set_points(&mut self, points: RangePoints) {
        // Round the handle places; the feathers follow exactly from them, so
        // a handle dragged into a corner stays exactly there.
        // Only points that moved are rounded, so a handle that was not touched
        // keeps its exact place.
        let before = self.points();
        let place = |now: f32, was: f32| {
            if now == was {
                was
            } else {
                round_to(now, DECIMALS)
            }
        };
        if points.fade_in != before.fade_in || points.full_from != before.full_from {
            if points.full_from != before.full_from {
                self.low = round_to(points.full_from, DECIMALS);
            }
            let fade_in = place(points.fade_in, before.fade_in);
            self.low_feather = (self.low - fade_in) / LUMINANCE_FEATHER_WIDTH;
        }
        if points.full_to != before.full_to || points.fade_out != before.fade_out {
            if points.full_to != before.full_to {
                self.high = round_to(points.full_to, DECIMALS);
            }
            let fade_out = place(points.fade_out, before.fade_out);
            self.high_feather = (fade_out - self.high) / LUMINANCE_FEATHER_WIDTH;
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
    let value_text = format!(
        "low {:.2}, high {:.2}, low feather {:.2}, high feather {:.2}",
        range.low, range.high, range.low_feather, range.high_feather
    );
    feathered_range::feathered_range_track(
        ui,
        range,
        "Luminance range",
        value_text,
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
    let (low, high) = (range.low, range.high);
    let field = |label, value, range, disabled_reason| RangeField {
        label,
        value,
        range,
        decimals: 2,
        speed: 0.005,
        disabled_reason,
    };
    feathered_range::range_fields(
        ui,
        vec![
            field("Low", &mut range.low, 0.0..=high, None),
            field("High", &mut range.high, low..=1.0, None),
            field(
                "Low feather",
                &mut range.low_feather,
                0.0..=LUMINANCE_FEATHER_MAX,
                (low <= 0.0).then_some("Raise Low above 0 to soften the dark edge."),
            ),
            field(
                "High feather",
                &mut range.high_feather,
                0.0..=LUMINANCE_FEATHER_MAX,
                (high >= 1.0).then_some("Lower High below 1 to soften the bright edge."),
            ),
        ],
    );
    *range != before
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::components::feathered_range::RangeHandle;
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
    fn handles_reach_every_corner_and_fades_can_span_the_whole_range() {
        // The dark fade can open all the way to black.
        let mut range = DEFAULT;
        drag_range(&mut range, &DEFAULT, RangeHandle::StartFeather, -1.0);
        assert!(range.points().fade_in.abs() < 1e-6, "{range:?}");
        assert!(range.weight(0.1) > 0.0 && range.weight(0.1) < 1.0);

        // A solid handle follows the pointer into the corner, squeezing its
        // fade, and the hollow handle stays reachable there.
        let mut range = DEFAULT;
        drag_range(&mut range, &DEFAULT, RangeHandle::Start, -1.0);
        assert_eq!(range.low, 0.0);
        assert!(range.points().fade_in.abs() < 1e-6);
        let mut reopened = range;
        drag_range(&mut reopened, &range, RangeHandle::Start, 0.3);
        assert!((reopened.low - 0.3).abs() < 1e-6);

        let mut range = DEFAULT;
        drag_range(&mut range, &DEFAULT, RangeHandle::EndFeather, 1.0);
        assert!((range.points().fade_out - 1.0).abs() < 1e-6);
        drag_range(&mut range, &DEFAULT, RangeHandle::End, 1.0);
        assert_eq!(range.high, 1.0);
    }

    #[test]
    fn crossing_handles_push_the_others_along() {
        let mut range = DEFAULT;
        drag_range(&mut range, &DEFAULT, RangeHandle::Start, 0.9);
        let points = range.points();
        assert!((points.full_from - 1.0).abs() < 1e-6, "{range:?}");
        assert!(points.full_from <= points.full_to && points.full_to <= points.fade_out);
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
