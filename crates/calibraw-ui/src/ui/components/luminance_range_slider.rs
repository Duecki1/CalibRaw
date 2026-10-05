//! The luminance-range mask on the shared feathered-range track, over a
//! black-to-white backdrop. Like the depth range, circles sit at the centre of
//! each soft edge and diamonds where selection reaches full strength (`low`
//! and `high`). The mask has one feather for both edges, so either diamond
//! changes it and both edges follow, each keeping its centre.

use super::feathered_range::{self, FeatheredRange, RangeHandle};
use crate::pipeline::luminance_range_weight;
use eframe::egui::{self, Color32, Ui};
use moduwu_design::NumberField;

/// Ramp width (in luminance) of a feather of 1, as the mask rasterizer uses.
const FEATHER_WIDTH: f32 = 0.35;
const DEFAULT: LuminanceRange = LuminanceRange {
    low: 0.2,
    high: 0.8,
    feather: 0.15,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LuminanceRange {
    pub(crate) low: f32,
    pub(crate) high: f32,
    pub(crate) feather: f32,
}

impl LuminanceRange {
    fn ramp(self) -> f32 {
        self.feather.clamp(0.0, 1.0) * FEATHER_WIDTH
    }

    fn start_center(self) -> f32 {
        self.low - self.ramp() * 0.5
    }

    fn end_center(self) -> f32 {
        self.high + self.ramp() * 0.5
    }

    /// Sets the shared ramp width while both edge centres stay put, limited
    /// so the bounds stay ordered and inside 0–1.
    fn with_ramp(self, ramp: f32) -> Self {
        let (start, end) = (self.start_center(), self.end_center());
        let lowest = 0.0_f32.max(-2.0 * start).max(2.0 * (end - 1.0));
        let highest = FEATHER_WIDTH.min((end - start).max(0.0));
        let ramp = ramp.max(lowest).min(highest);
        if !ramp.is_finite() || lowest > highest {
            return self;
        }
        Self {
            low: start + ramp * 0.5,
            high: end - ramp * 0.5,
            feather: ramp / FEATHER_WIDTH,
        }
    }
}

impl FeatheredRange for LuminanceRange {
    fn handle_value(&self, handle: RangeHandle) -> f32 {
        match handle {
            RangeHandle::Start => self.start_center(),
            RangeHandle::End => self.end_center(),
            RangeHandle::StartFeather => self.low,
            RangeHandle::EndFeather => self.high,
        }
    }

    /// An edge centred beyond black or white has no visible ramp to adjust.
    fn handle_active(&self, handle: RangeHandle) -> bool {
        match handle {
            RangeHandle::Start | RangeHandle::End => true,
            RangeHandle::StartFeather => self.start_center() > 0.0,
            RangeHandle::EndFeather => self.end_center() < 1.0,
        }
    }

    fn drag(&mut self, start: &Self, handle: RangeHandle, delta: f32) {
        *self = match handle {
            RangeHandle::Start => Self {
                low: (start.low + delta).max(0.0).min(start.high),
                ..*start
            },
            RangeHandle::End => Self {
                high: (start.high + delta).max(start.low).min(1.0),
                ..*start
            },
            RangeHandle::StartFeather => {
                start.with_ramp(2.0 * (start.low + delta - start.start_center()))
            }
            RangeHandle::EndFeather => {
                start.with_ramp(2.0 * (start.end_center() - (start.high + delta)))
            }
        };
    }

    fn weight(&self, t: f32) -> f32 {
        luminance_range_weight(t, self.low, self.high, self.feather)
    }

    fn reset(&mut self) {
        *self = DEFAULT;
    }
}

/// Shows the luminance range and returns whether it changed.
pub(crate) fn luminance_range_slider(ui: &mut Ui, range: &mut LuminanceRange) -> bool {
    let before = *range;
    ui.strong("Luminance range");
    let value_text = format!(
        "low {:.2}, high {:.2}, feather {:.2}",
        range.low, range.high, range.feather
    );
    feathered_range::feathered_range_track(
        ui,
        range,
        "Luminance range",
        value_text,
        "Drag the lower circles to move the dark and bright edges of the selection. Drag an upper diamond to soften both edges. Double-click to reset.",
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
    ui.columns(2, |columns| {
        columns[0].horizontal(|ui| {
            ui.label("Low");
            ui.add(
                NumberField::new(&mut range.low, 0.0..=range.high)
                    .speed(0.005)
                    .decimals(2),
            );
        });
        columns[1].horizontal(|ui| {
            ui.label("High");
            ui.add(
                NumberField::new(&mut range.high, range.low..=1.0)
                    .speed(0.005)
                    .decimals(2),
            );
        });
    });
    *range != before
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_valid(range: LuminanceRange, context: &str) {
        assert!(
            range.low >= -1e-6 && range.high <= 1.0 + 1e-6,
            "{context}: {range:?}"
        );
        assert!(range.low <= range.high + 1e-6, "{context}: {range:?}");
        assert!(
            (-1e-6..=1.0 + 1e-6).contains(&range.feather),
            "{context}: {range:?}"
        );
    }

    #[test]
    fn handles_sit_at_edge_centres_and_bounds() {
        let range = DEFAULT;
        let ramp = 0.15 * FEATHER_WIDTH;
        assert!((range.handle_value(RangeHandle::Start) - (0.2 - ramp * 0.5)).abs() < 1e-6);
        assert!((range.handle_value(RangeHandle::End) - (0.8 + ramp * 0.5)).abs() < 1e-6);
        assert_eq!(range.handle_value(RangeHandle::StartFeather), 0.2);
        assert_eq!(range.handle_value(RangeHandle::EndFeather), 0.8);
        for step in 0..=20 {
            let t = step as f32 / 20.0;
            assert_eq!(range.weight(t), luminance_range_weight(t, 0.2, 0.8, 0.15));
        }
    }

    #[test]
    fn circles_move_one_bound_and_diamonds_change_the_shared_feather() {
        let start = DEFAULT;
        let mut range = start;
        range.drag(&start, RangeHandle::Start, 0.1);
        assert!((range.low - 0.3).abs() < 1e-6);
        assert_eq!((range.high, range.feather), (start.high, start.feather));

        let mut range = start;
        range.drag(&start, RangeHandle::StartFeather, 0.02);
        assert!(range.feather > start.feather);
        assert!((range.start_center() - start.start_center()).abs() < 1e-6);
        assert!((range.end_center() - start.end_center()).abs() < 1e-6);
        assert!((range.low - 0.22).abs() < 1e-5);
    }

    #[test]
    fn every_drag_keeps_the_range_valid() {
        for start in [
            DEFAULT,
            LuminanceRange {
                low: 0.0,
                high: 1.0,
                feather: 1.0,
            },
            LuminanceRange {
                low: 0.5,
                high: 0.5,
                feather: 0.0,
            },
        ] {
            for handle in RangeHandle::ALL {
                for delta in [-2.0, -0.2, -0.01, 0.0, 0.01, 0.2, 2.0] {
                    let mut range = start;
                    range.drag(&start, handle, delta);
                    assert_valid(range, &format!("{handle:?} {delta} from {start:?}"));
                }
            }
        }
    }
}
