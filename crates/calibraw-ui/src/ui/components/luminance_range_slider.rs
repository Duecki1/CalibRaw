//! The luminance-range mask on the shared feathered-range track, over a
//! black-to-white backdrop. Solid handles sit at `low` and `high`, where
//! selection is full; hollow handles where each edge's own fade begins. The
//! track is CIE lightness ([`luminance_to_lightness`]), so shadows get as
//! much of it as highlights; the bounds are stored as linear luminance.

use super::feathered_range::{
    self, round_to, EndEdit, FeatheredRange, RangeEdit, RangeHandle, RangePoints,
};
use crate::pipeline::{
    lightness_to_luminance, luminance_range_weight, luminance_to_lightness, LUMINANCE_FEATHER_MAX,
    LUMINANCE_FEATHER_WIDTH,
};
use eframe::egui::{self, Color32, Ui};

/// Stored precision of dragged linear-luminance bounds, fine enough for
/// one lightness step in deep shadows.
const DECIMALS: i32 = 4;
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
        let low = luminance_to_lightness(self.low);
        let high = luminance_to_lightness(self.high);
        RangePoints {
            fade_in: low - ramp(self.low_feather),
            full_from: low,
            full_to: high,
            fade_out: high + ramp(self.high_feather),
        }
    }

    fn set_points(&mut self, edit: RangeEdit) {
        if let Some(edit) = edit.start {
            apply_end_edit(edit, &mut self.low, &mut self.low_feather, -1.0);
        }
        if let Some(edit) = edit.end {
            apply_end_edit(edit, &mut self.high, &mut self.high_feather, 1.0);
        }
    }

    /// Bounds read as lightness (0–100), the scale of the track.
    fn handle_text(&self, handle: RangeHandle) -> String {
        let lightness = |luminance| luminance_to_lightness(luminance) * 100.0;
        match handle {
            RangeHandle::Start => format!("Low {:.0}", lightness(self.low)),
            RangeHandle::End => format!("High {:.0}", lightness(self.high)),
            RangeHandle::StartFeather => format!("Low feather {:.2}", self.low_feather),
            RangeHandle::EndFeather => format!("High feather {:.2}", self.high_feather),
        }
    }

    fn weight(&self, t: f32) -> f32 {
        luminance_range_weight(
            lightness_to_luminance(t),
            self.low,
            self.high,
            [self.low_feather, self.high_feather],
        )
    }

    fn reset(&mut self) {
        *self = DEFAULT;
    }
}

/// Stores one end's drag: the bound as linear luminance, and the feather from
/// the fade's track place, which may lie past black or white. `direction` is
/// -1 for the low end, whose fade lies below its bound, and 1 for the high
/// end.
fn apply_end_edit(edit: EndEdit, bound: &mut f32, feather: &mut f32, direction: f32) {
    let store = |place| round_to(lightness_to_luminance(place), DECIMALS);
    let fade = match edit {
        EndEdit::Moved { full } => {
            *bound = store(full);
            return;
        }
        EndEdit::Faded { fade, .. } => fade,
        EndEdit::Squeezed { full, fade } => {
            *bound = store(full);
            fade
        }
    };
    let fade = round_to(fade, DECIMALS);
    *feather = direction * (fade - luminance_to_lightness(*bound)) / LUMINANCE_FEATHER_WIDTH;
}

/// Shows the luminance range and returns whether it changed.
pub(crate) fn luminance_range_slider(ui: &mut Ui, range: &mut LuminanceRange) -> bool {
    let before = *range;
    ui.label("Luminance range");
    feathered_range::feathered_range_track(ui, range, "Luminance range", |painter, track| {
        const COLUMNS: usize = 64;
        let mut mesh = egui::Mesh::default();
        for column in 0..=COLUMNS {
            let t = column as f32 / COLUMNS as f32;
            let gray = Color32::from_gray(egui::ecolor::gamma_u8_from_linear_f32(
                lightness_to_luminance(t),
            ));
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
    });
    *range != before
}
