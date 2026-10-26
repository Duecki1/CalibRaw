use super::feathered_range::{self, round_to, FeatheredRange, RangeEdit, RangeHandle, RangePoints};
use crate::pipeline::DepthRangeSettings;
use eframe::egui::{Response, Ui};

/// Stored precision of dragged values.
const DECIMALS: i32 = 3;

impl FeatheredRange for DepthRangeSettings {
    /// `near` and `far` are ramp centres and the feathers ramp widths. A
    /// fade may run off the track; it is drawn at the track end. (An end at
    /// exactly 0 or 1 has no ramp at all; its stored feather is kept for when
    /// the end moves inwards again.)
    fn points(&self) -> RangePoints {
        let near_half = self.near_feather.clamp(0.0, 1.0) * 0.5;
        let far_half = self.far_feather.clamp(0.0, 1.0) * 0.5;
        RangePoints {
            fade_in: self.near - near_half,
            full_from: self.near + near_half,
            full_to: self.far - far_half,
            fade_out: self.far + far_half,
        }
    }

    fn set_points(&mut self, edit: RangeEdit) {
        let round = |value| round_to(value, DECIMALS);
        if let Some(edit) = edit.start {
            let half = self.near_feather.clamp(0.0, 1.0) * 0.5;
            match edit.resolve(self.near + half, round) {
                (full_from, None) => self.near = full_from - half,
                (full_from, Some(fade_in)) => {
                    self.near_feather = full_from - fade_in;
                    self.near = (fade_in + full_from) * 0.5;
                }
            }
        }
        if let Some(edit) = edit.end {
            let half = self.far_feather.clamp(0.0, 1.0) * 0.5;
            match edit.resolve(self.far - half, round) {
                (full_to, None) => self.far = full_to + half,
                (full_to, Some(fade_out)) => {
                    self.far_feather = fade_out - full_to;
                    self.far = (full_to + fade_out) * 0.5;
                }
            }
        }
        self.near = self.near.max(0.0);
        self.far = self.far.min(1.0);
        // Rounding a moved point must not leave Near a hair past Far. The end
        // this step changed yields (Far, after a push) so an untouched end stays exact.
        if self.near > self.far {
            if edit.end.is_some() {
                self.far = self.near;
            } else {
                self.near = self.far;
            }
        }
    }

    fn handle_text(&self, handle: RangeHandle) -> String {
        match handle {
            RangeHandle::Start => format!("Near {:.2}", self.near),
            RangeHandle::End => format!("Far {:.2}", self.far),
            RangeHandle::StartFeather => format!("Near feather {:.2}", self.near_feather),
            RangeHandle::EndFeather => format!("Far feather {:.2}", self.far_feather),
        }
    }

    /// Near and Far stay on the track: a fade already off it (older
    /// settings) that a solid handle squeezes narrows further, so Near and
    /// Far still reach the track ends. Near must not pass Far (the mask
    /// clamps Far to Near), while the two fades may overlap. A dragged handle
    /// that would carry one centre past the other stops where they meet.
    fn constrain(&self, points: RangePoints, moved: RangeHandle) -> RangePoints {
        let mut limited = points;
        limited.fade_in = points.fade_in.max(-points.full_from);
        limited.fade_out = points.fade_out.min(2.0 - points.full_to);
        let near = (limited.fade_in + limited.full_from) * 0.5;
        let far = (limited.full_to + limited.fade_out) * 0.5;
        if near <= far {
            return limited;
        }
        match moved {
            // A fade handle moves alone.
            RangeHandle::StartFeather => {
                limited.fade_in = (2.0 * far - limited.full_from).clamp(0.0, limited.full_from);
            }
            RangeHandle::EndFeather => {
                limited.fade_out = (2.0 * near - limited.full_to).clamp(limited.full_to, 1.0);
            }
            // A solid handle moves its end, fade included, back to the meeting point.
            RangeHandle::Start => {
                limited.fade_in -= near - far;
                limited.full_from -= near - far;
            }
            RangeHandle::End => {
                limited.full_to += near - far;
                limited.fade_out += near - far;
            }
        }
        limited
    }

    fn weight(&self, t: f32) -> f32 {
        DepthRangeSettings::weight(self, t)
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

fn range_track(ui: &mut Ui, range: &mut DepthRangeSettings) -> Response {
    let background = feathered_range::plain_track_background(ui.visuals());
    feathered_range::feathered_range_track(ui, range, "Depth range", background)
}

pub(crate) fn depth_range_slider(ui: &mut Ui, range: &mut DepthRangeSettings) -> bool {
    let before = *range;
    ui.label("Depth range");
    range_track(ui, range);
    *range != before
}
