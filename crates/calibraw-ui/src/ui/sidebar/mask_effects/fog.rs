use super::{
    effect_card, effect_color, effect_details, float_param_slider, image_lights_toggle,
    pattern_seed,
};
use crate::pipeline::{effect_params::fog, FogEffectSettings, MaskEffect};
use crate::ui::components::feathered_range::{
    self, EndEdit, FeatheredRange, RangeEdit, RangeHandle, RangePoints,
};
use eframe::egui::{self, Ui};

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
        onset_width(self.settings.softness)
    }
}

/// The onset width (scene depth) at `softness` (0–100).
fn onset_width(softness: f32) -> f32 {
    egui::lerp(
        ONSET_WIDTH[0]..=ONSET_WIDTH[1],
        (softness / 100.0).clamp(0.0, 1.0),
    )
}

/// The whole Softness whose onset width is nearest `width`.
fn softness_for(width: f32) -> f32 {
    let softness = (width - ONSET_WIDTH[0]) / (ONSET_WIDTH[1] - ONSET_WIDTH[0]) * 100.0;
    fog::SOFTNESS.clamp(softness.round())
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

    /// Fog start (the hollow handle) moves alone and stops at the onset
    /// widths Softness can express. The solid handle carries Fog start along
    /// and leaves Softness unchanged, unless the drag squeezed the onset at
    /// the near end; Softness then narrows, down to Softness 0. Fog start and
    /// Softness are whole numbers, chosen here so that storing them moves
    /// nothing else. So the solid handle moves in 1% steps of Fog start and
    /// sits on the stored curve, within half a step of the pointer.
    fn constrain(&self, points: RangePoints, moved: RangeHandle) -> RangePoints {
        let max_start = fog::START.max / 100.0;
        let (start, full) = if moved == RangeHandle::StartFeather {
            // Fog start moves alone, within the onset widths Softness allows.
            let full = points.full_from;
            let lowest = (full - ONSET_WIDTH[1]).max(0.0);
            let highest = (full - ONSET_WIDTH[0]).min(max_start);
            let mut start = (points.fade_in.clamp(lowest, highest) * 100.0).round() / 100.0;
            if start > highest {
                start -= 0.01;
            }
            if start < lowest {
                start += 0.01;
            }
            (start, full)
        } else {
            // The solid handle carries Fog start along with the stored width,
            // unless the drag squeezed the onset against the near end.
            let carried = points.full_from - points.fade_in;
            let width = if points.fade_in <= 0.0 && carried < self.onset_width() {
                onset_width(softness_for(carried))
            } else {
                self.onset_width()
            };
            let start =
                (((points.full_from - width) * 100.0).round() / 100.0).clamp(0.0, max_start);
            (start, start + width)
        };
        RangePoints {
            fade_in: start.clamp(0.0, max_start),
            full_from: full.min(1.0),
            full_to: 1.0,
            fade_out: 1.0,
        }
    }

    /// Stores whole numbers, as the sliders show them. Fog stores where the
    /// onset begins and its width, so the full-strength point is taken from
    /// the drag even when only Fog start moved.
    fn set_points(&mut self, edit: RangeEdit) {
        let start = match edit.start {
            None => return,
            Some(EndEdit::Moved { full }) => full - self.onset_width(),
            Some(EndEdit::Faded { full, fade } | EndEdit::Squeezed { full, fade }) => {
                self.settings.softness = softness_for(full - fade);
                fade
            }
        };
        self.settings.start = fog::START.clamp((start * 100.0).round());
    }

    fn handle_text(&self, handle: RangeHandle) -> String {
        match handle {
            RangeHandle::StartFeather => {
                format!("{} {:.0}", fog::START.label, self.settings.start)
            }
            _ => format!(
                "Full strength at {:.0}, {} {:.0}",
                self.points().full_from * 100.0,
                fog::SOFTNESS.label,
                self.settings.softness
            ),
        }
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
    ui.label(fog::START.label);
    feathered_range::feathered_range_track(
        ui,
        &mut onset,
        fog::START.label,
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
            changed |= float_param_slider(ui, &mut settings.light_glow, fog::LIGHT_GLOW);
            changed |= image_lights_toggle(ui, &mut settings.image_lights);
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

    /// The solid handle's place.
    fn solid(fog: &FogOnset) -> f32 {
        fog.points().full_from
    }

    /// Half a step of Fog start (1%): how far the solid handle may sit from
    /// the pointer, plus float error.
    const HALF_STEP: f32 = 0.005 + 1e-6;

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
    fn the_solid_handle_carries_fog_start_and_keeps_softness() {
        let start = onset();
        let mut fog = start;
        let mut drag = HandleDrag::new(&fog, RangeHandle::Start);
        // It snaps to whole Fog start percentages, within half a step of the
        // pointer, and Softness stays as it was.
        for step in 0..=60 {
            let target = 0.3 + step as f32 * 0.0073;
            drag.move_to(&mut fog, target);
            assert!(
                (solid(&fog) - target).abs() <= HALF_STEP,
                "{target}: {fog:?}"
            );
            assert_eq!(fog.settings.softness, start.settings.softness);
            assert_eq!(fog.settings.start, fog.settings.start.round());
        }
        // Towards the near end it carries Fog start to 0, then narrows the
        // onset down to Softness 0.
        let width = start.onset_width();
        drag.move_to(&mut fog, width);
        assert_eq!(fog.settings.start, 0.0);
        assert_eq!(fog.settings.softness, start.settings.softness);
        drag.move_to(&mut fog, 0.05);
        assert_eq!(fog.settings.start, 0.0);
        assert!(fog.settings.softness < start.settings.softness, "{fog:?}");
        assert!((solid(&fog) - 0.05).abs() < 1e-3, "{fog:?}");
        drag.move_to(&mut fog, 0.0);
        assert_eq!((fog.settings.start, fog.settings.softness), (0.0, 0.0));
        assert!((solid(&fog) - ONSET_WIDTH[0]).abs() < 1e-6, "{fog:?}");
    }

    #[test]
    fn the_solid_handle_keeps_fog_start_at_its_maximum() {
        let start = FogOnset {
            settings: FogEffectSettings {
                start: 80.0,
                softness: 0.0,
                ..Default::default()
            },
        };
        let mut fog = start;
        drag_range(&mut fog, &start, RangeHandle::Start, 0.5);
        assert_eq!(fog.settings.start, 95.0);
        assert_eq!(fog.settings.softness, 0.0);
        let moved = fog;
        drag_range(&mut fog, &moved, RangeHandle::Start, 0.0);
        assert_eq!(fog.settings.start, 95.0);
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
