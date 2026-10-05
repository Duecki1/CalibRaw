use super::{effect_card, effect_color, effect_details, float_param_slider, pattern_seed};
use crate::pipeline::{effect_params::fog, FogEffectSettings, MaskEffect};
use crate::ui::components::feathered_range::{self, FeatheredRange, RangeHandle};
use eframe::egui::{self, Align, Layout, Ui};
use moduwu_design::NumberField;

/// Onset widths (scene depth) at Softness 0 and 100, as `apply_fog` uses.
const ONSET_WIDTH: [f32; 2] = [0.025, 0.18];

/// Fog onset on the shared feathered-range track, near side only: the circle
/// is where fog starts (Fog start) and the diamond where it reaches its full
/// rate (Softness). The curve is the fog's opacity against scene distance for
/// the centre of the frame, without the random bank variation.
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

/// Fog travelled past `start`, with a smooth onset of `width`. Mirrors
/// `fog_onset_integral` in `mask_effects/atmosphere.wgsl`.
fn fog_onset_integral(distance: f32, start: f32, width: f32) -> f32 {
    let travel = (distance - start).max(0.0);
    let u = (travel / width).clamp(0.0, 1.0);
    width * (u * u * u - 0.5 * u * u * u * u) + (travel - width).max(0.0)
}

impl FeatheredRange for FogOnset {
    fn handle_value(&self, handle: RangeHandle) -> f32 {
        match handle {
            RangeHandle::Start => self.start(),
            RangeHandle::StartFeather => self.start() + self.onset_width(),
            RangeHandle::End | RangeHandle::EndFeather => 1.0,
        }
    }

    fn handle_active(&self, handle: RangeHandle) -> bool {
        matches!(handle, RangeHandle::Start | RangeHandle::StartFeather)
    }

    fn drag(&mut self, start: &Self, handle: RangeHandle, delta: f32) {
        match handle {
            RangeHandle::Start => {
                self.settings.start = fog::START.clamp((start.start() + delta) * 100.0);
            }
            RangeHandle::StartFeather => {
                let width = start.onset_width() + delta;
                let softness = (width - ONSET_WIDTH[0]) / (ONSET_WIDTH[1] - ONSET_WIDTH[0]) * 100.0;
                self.settings.softness = fog::SOFTNESS.clamp(softness);
            }
            RangeHandle::End | RangeHandle::EndFeather => {}
        }
    }

    /// Opacity of the fog in front of a surface at scene distance `t`, as
    /// `apply_fog` computes it for a centre ray.
    fn weight(&self, t: f32) -> f32 {
        let amount = (self.settings.amount / 100.0).clamp(0.0, 1.0);
        let density = (self.settings.density / 100.0).clamp(0.0, 1.0);
        let influence = (self.settings.depth_influence / 100.0).clamp(0.0, 1.0);
        let distance = egui::lerp(1.0..=t, influence);
        let start = self.start();
        if distance <= start {
            return 0.0;
        }
        let optical_length = fog_onset_integral(distance, start, self.onset_width());
        1.0 - (-6.0 * density * density * amount * optical_length).exp()
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
        "How fog builds up with scene distance, near on the left. Drag the circle to keep the nearest part of the scene clear; drag the diamond to make the fog begin more softly or abruptly. Double-click to reset.",
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

    fn onset() -> FogOnset {
        FogOnset {
            settings: FogEffectSettings::default(),
        }
    }

    #[test]
    fn handles_set_start_and_softness_only() {
        let start = onset();
        let mut moved = start;
        moved.drag(&start, RangeHandle::Start, 0.1);
        assert!((moved.settings.start - (start.settings.start + 10.0)).abs() < 1e-4);
        assert_eq!(moved.settings.softness, start.settings.softness);

        let mut moved = start;
        moved.drag(&start, RangeHandle::StartFeather, -1.0);
        assert_eq!(moved.settings.softness, 0.0);
        assert_eq!(moved.settings.start, start.settings.start);
        moved.drag(&start, RangeHandle::StartFeather, 1.0);
        assert_eq!(moved.settings.softness, 100.0);

        let mut moved = start;
        moved.drag(&start, RangeHandle::Start, 5.0);
        assert_eq!(moved.settings.start, fog::START.max);
        assert!(!start.handle_active(RangeHandle::End));
        assert!(!start.handle_active(RangeHandle::EndFeather));
    }

    #[test]
    fn curve_is_clear_before_start_and_thickens_with_distance() {
        let fog = onset();
        assert_eq!(fog.weight(fog.start() * 0.5), 0.0);
        let mut previous = 0.0;
        for step in 0..=20 {
            let weight = fog.weight(step as f32 / 20.0);
            assert!((0.0..1.0).contains(&weight));
            assert!(weight >= previous);
            previous = weight;
        }
        assert!(previous > 0.0);
    }

    #[test]
    fn no_depth_influence_is_a_flat_veil() {
        let mut fog = onset();
        fog.settings.depth_influence = 0.0;
        assert_eq!(fog.weight(0.1), fog.weight(0.9));
    }
}
