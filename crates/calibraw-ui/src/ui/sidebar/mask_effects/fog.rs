use super::{effect_card, effect_color, effect_details, float_param_slider, pattern_seed};
use crate::pipeline::{effect_params::fog, FogEffectSettings, MaskEffect};
use crate::ui::components::feathered_range::{self, FeatheredRange, RangeHandle, RangePoints};
use eframe::egui::{self, Align, Layout, Ui};
use moduwu_design::NumberField;

/// Onset widths (scene depth) at Softness 0 and 100, as `apply_fog` uses.
const ONSET_WIDTH: [f32; 2] = [0.025, 0.18];

/// Fog onset on the shared feathered-range track, near side only. The curve
/// is how strongly fog builds up with scene distance: none before Fog start
/// (the hollow handle), rising over the onset width that Softness sets (as
/// `apply_fog`'s `fog_onset_integral` does) to its full rate at the solid
/// handle.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FogOnset {
    settings: FogEffectSettings,
}

impl FogOnset {
    fn start(self) -> f32 {
        fog::START.clamp(self.settings.start) / 100.0
    }

    fn onset_width(self) -> f32 {
        let softness = (self.settings.softness / 100.0).clamp(0.0, 1.0);
        egui::lerp(ONSET_WIDTH[0]..=ONSET_WIDTH[1], softness)
    }
}

impl FeatheredRange for FogOnset {
    const HAS_END: bool = false;

    fn points(&self) -> RangePoints {
        RangePoints {
            fade_in: self.start(),
            full_from: (self.start() + self.onset_width()).min(1.0),
            full_to: 1.0,
            fade_out: 1.0,
        }
    }

    /// The onset width is limited to what Softness can express. Fog start
    /// (the hollow handle) stops at those limits and never moves the solid
    /// handle; a dragged solid handle takes Fog start along when it must.
    /// Fog start is a whole percentage, chosen here so that storing it cannot
    /// move the solid handle either.
    fn constrain(&self, points: RangePoints, moved: RangeHandle) -> RangePoints {
        let full = points.full_from.max(ONSET_WIDTH[0]);
        let wanted = if moved == RangeHandle::StartFeather {
            points.fade_in
        } else {
            full - (full - points.fade_in).clamp(ONSET_WIDTH[0], ONSET_WIDTH[1])
        };
        let lowest = (full - ONSET_WIDTH[1]).max(0.0);
        let highest = (full - ONSET_WIDTH[0]).min(fog::START.max / 100.0);
        let mut start = (wanted.clamp(lowest, highest) * 100.0).round() / 100.0;
        if start > highest {
            start -= 0.01;
        }
        if start < lowest {
            start += 0.01;
        }
        RangePoints {
            fade_in: start.clamp(0.0, fog::START.max / 100.0),
            full_from: full.min(1.0),
            full_to: 1.0,
            fade_out: 1.0,
        }
    }

    /// Stores whole numbers, as the fields and sliders show them.
    fn set_points(&mut self, points: RangePoints) {
        self.settings.start = fog::START.clamp((points.fade_in * 100.0).round());
        let width = points.full_from - points.fade_in;
        let softness = (width - ONSET_WIDTH[0]) / (ONSET_WIDTH[1] - ONSET_WIDTH[0]) * 100.0;
        self.settings.softness = fog::SOFTNESS.clamp(softness.round());
    }

    /// The rate at which fog accumulates at scene distance `t`, relative to
    /// its full rate: the derivative of `fog_onset_integral`.
    fn weight(&self, t: f32) -> f32 {
        let u = ((t - self.start()) / self.onset_width()).clamp(0.0, 1.0);
        u * u * (3.0 - 2.0 * u)
    }

    fn reset(&mut self) {
        self.settings.start = fog::START.default;
        self.settings.softness = fog::SOFTNESS.default;
    }
}

fn fog_onset(ui: &mut Ui, settings: &mut FogEffectSettings) -> bool {
    let before = *settings;
    let mut onset = FogOnset {
        settings: *settings,
    };
    ui.horizontal(|ui| {
        ui.label(fog::START.label);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add(
                NumberField::new(&mut onset.settings.start, fog::START.range())
                    .speed(fog::START.step)
                    .decimals(fog::START.decimals),
            );
        });
    });
    let value_text = format!(
        "start {:.0}, softness {:.0}",
        onset.settings.start, onset.settings.softness
    );
    feathered_range::feathered_range_track(
        ui,
        &mut onset,
        fog::START.label,
        value_text,
        "How fog builds up with scene distance, near on the left. The hollow handle is where fog starts; the solid handle is where it reaches full strength.",
        feathered_range::plain_track_background(ui.visuals()),
    );
    settings.start = onset.settings.start;
    settings.softness = onset.settings.softness;
    *settings != before
}

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut FogEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
) -> bool {
    effect_card(
        ui,
        MaskEffect::Fog,
        settings,
        enabled,
        remove,
        |ui, settings| {
            let mut changed = float_param_slider(ui, &mut settings.amount, fog::AMOUNT);
            changed |= float_param_slider(ui, &mut settings.density, fog::DENSITY);
            if moduwu_design::toggle_button(ui, "Scene depth", settings.depth_enabled)
                .on_hover_text("Generate and use shared scene depth so distant areas collect more fog. Turn off to use a flat veil; depth settings are preserved.")
                .clicked()
            {
                settings.depth_enabled = !settings.depth_enabled;
                changed = true;
            }
            if settings.depth_enabled {
                changed |= fog_onset(ui, settings);
            }
            changed |= effect_color(ui, "fog-color-picker", &mut settings.color, fog::COLOR);
            changed |= effect_details(ui, "Atmosphere details", |ui| {
                let mut changed = false;
                if settings.depth_enabled {
                    changed |=
                        float_param_slider(ui, &mut settings.depth_influence, fog::DEPTH_INFLUENCE);
                }
                changed |= float_param_slider(ui, &mut settings.softness, fog::SOFTNESS);
                changed |= float_param_slider(ui, &mut settings.variation, fog::VARIATION);
                if settings.variation > 0.0 {
                    changed |= float_param_slider(ui, &mut settings.scale, fog::SCALE);
                    changed |= pattern_seed(ui, &mut settings.seed, fog::SEED);
                }
                changed
            });
            changed
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::components::feathered_range::{drag_range, HandleDrag};

    fn onset() -> FogOnset {
        FogOnset {
            settings: FogEffectSettings::default(),
        }
    }

    /// The solid handle's place, to within one Softness step of storage.
    fn solid(fog: &FogOnset) -> f32 {
        fog.points().full_from
    }

    #[test]
    fn fog_start_never_moves_the_solid_handle() {
        // Fog in the middle with the widest onset (0.50 to 0.68).
        let mut fog = FogOnset {
            settings: FogEffectSettings {
                start: 50.0,
                softness: 100.0,
                ..Default::default()
            },
        };
        let mut drag = HandleDrag::new(&fog, RangeHandle::StartFeather);
        // Left: the onset is already at its widest, so Fog start stays.
        for target in [0.45, 0.30, 0.0] {
            drag.move_to(&mut fog, target);
            assert!((solid(&fog) - 0.68).abs() < 1e-3, "{target}: {fog:?}");
        }
        assert_eq!(fog.settings.start, 50.0);
        // Right: Fog start follows until the narrowest onset, then stops.
        for target in [0.55, 0.60, 0.66, 0.9] {
            drag.move_to(&mut fog, target);
            assert!((solid(&fog) - 0.68).abs() < 1e-3, "{target}: {fog:?}");
        }
        assert_eq!(fog.settings.start, 65.0);
        // And back again, still without moving the solid handle.
        drag.move_to(&mut fog, 0.52);
        assert_eq!(fog.settings.start, 52.0);
        assert!((solid(&fog) - 0.68).abs() < 1e-3, "{fog:?}");
    }

    #[test]
    fn the_solid_handle_takes_fog_start_along_beyond_the_softness_limits() {
        let start = onset();
        let mut fog = start;
        let mut drag = HandleDrag::new(&fog, RangeHandle::Start);
        drag.move_to(&mut fog, 0.5);
        assert!((solid(&fog) - 0.5).abs() < 1e-3, "{fog:?}");
        assert_eq!(fog.settings.softness, 100.0);
        assert_eq!(fog.settings.start, 32.0);
        drag.move_to(&mut fog, 0.0);
        assert_eq!(fog.settings.start, 0.0);
    }

    #[test]
    fn curve_is_clear_before_start_and_full_at_the_solid_handle() {
        let fog = onset();
        let points = fog.points();
        assert_eq!(fog.weight(points.fade_in * 0.5), 0.0);
        assert_eq!(fog.weight(points.fade_in), 0.0);
        assert_eq!(fog.weight(points.full_from), 1.0);
    }

    #[test]
    fn dragged_values_are_whole_numbers() {
        let start = onset();
        let mut moved = start;
        drag_range(&mut moved, &start, RangeHandle::StartFeather, 0.0365);
        assert_eq!(moved.settings.start, moved.settings.start.round());
        drag_range(&mut moved, &start, RangeHandle::Start, -0.0123);
        assert_eq!(moved.settings.softness, moved.settings.softness.round());
    }
}
