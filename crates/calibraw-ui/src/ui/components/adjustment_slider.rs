//! CalibRaw's adjustment slider: [`moduwu_design::Slider`] configured from
//! parameter specifications, with photographic track gradients.

use crate::pipeline::effect_params::FloatParamSpec;
use eframe::egui::{self, Ui};
use moduwu_design::Slider;
use std::ops::RangeInclusive;

/// Colours painted along a slider track.
#[derive(Clone, Copy, Debug)]
pub(crate) enum SliderGradient {
    HueDegrees {
        start: f32,
        end: f32,
    },
    ChannelHue {
        left: egui::Color32,
        center: egui::Color32,
        right: egui::Color32,
    },
    Brightness,
    Temperature,
    Tint,
    CameraTint {
        neutral_fraction: f32,
    },
    Colorfulness,
    Saturation(egui::Color32),
    Luminance(egui::Color32),
}

/// A labelled numeric slider with an editable value field.
///
/// Double-clicking the label or track resets to [`Self::reset_to`] (zero by
/// default), never to the value the widget was first shown with.
#[must_use = "call `show` to render the slider"]
pub(crate) struct AdjustmentSlider<'a, Num> {
    slider: Slider<'a, Num>,
}

impl<'a, Num> AdjustmentSlider<'a, Num>
where
    Num: egui::emath::Numeric + Copy,
{
    pub(crate) fn new(label: &'a str, value: &'a mut Num, range: RangeInclusive<Num>) -> Self {
        Self {
            slider: Slider::new(label, value, range),
        }
    }

    /// Decimal places shown and stored.
    pub(crate) fn decimals(self, decimals: usize) -> Self {
        Self {
            slider: self.slider.decimals(decimals),
        }
    }

    /// Value change per arrow-key press or dragged point in the value field.
    pub(crate) fn step(self, step: f64) -> Self {
        Self {
            slider: self.slider.step(step),
        }
    }

    pub(crate) fn hover_text(self, hover_text: impl Into<Option<&'a str>>) -> Self {
        Self {
            slider: self.slider.hover_text(hover_text),
        }
    }

    pub(crate) fn reset_to(self, reset_value: Num) -> Self {
        Self {
            slider: self.slider.reset_to(reset_value),
        }
    }

    pub(crate) fn gradient(self, gradient: SliderGradient) -> Self {
        Self {
            slider: self
                .slider
                .gradient(move |fraction| gradient_color_at(gradient, fraction)),
        }
    }

    pub(crate) fn accent(self, accent: egui::Color32) -> Self {
        Self {
            slider: self.slider.accent(accent),
        }
    }

    /// Renders the full labelled row and returns whether the value changed.
    pub(crate) fn show(self, ui: &mut Ui) -> bool {
        self.slider.show(ui)
    }

    /// Renders only the track at `width`, using the label as the widget id.
    #[cfg(any(not(target_os = "android"), test))]
    pub(crate) fn show_inline(self, ui: &mut Ui, width: f32) -> moduwu_design::SliderResponse {
        self.slider.show_track(ui, width)
    }
}

impl<'a> AdjustmentSlider<'a, f32> {
    /// A resettable slider configured from a shared parameter specification.
    pub(crate) fn from_spec(value: &'a mut f32, spec: FloatParamSpec) -> Self {
        Self::new(spec.label, value, spec.range())
            .decimals(spec.decimals)
            .step(spec.step)
            .hover_text(spec.tooltip)
            .reset_to(spec.default)
    }
}

pub(crate) fn hue_adjustment_slider(
    ui: &mut Ui,
    value: &mut f32,
    hover_text: Option<&str>,
) -> bool {
    let spec = crate::pipeline::effect_params::adjustment::HUE;
    AdjustmentSlider::from_spec(value, spec)
        .hover_text(hover_text)
        .gradient(SliderGradient::HueDegrees {
            start: spec.min,
            end: spec.max,
        })
        .show(ui)
}

pub(crate) fn float_param_slider(ui: &mut Ui, value: &mut f32, spec: FloatParamSpec) -> bool {
    AdjustmentSlider::from_spec(value, spec).show(ui)
}

/// A direction parameter in degrees (0° right, clockwise positive, as the
/// effect shaders use) on a [`moduwu_design::AngleDial`].
pub(crate) fn float_param_angle(ui: &mut Ui, value: &mut f32, spec: FloatParamSpec) -> bool {
    moduwu_design::AngleDial::new(spec.label, value)
        .range(spec.range())
        .decimals(spec.decimals)
        .step(spec.step as f32)
        .reset_to(spec.default)
        .hover_text(spec.tooltip)
        .show(ui)
}

pub(crate) fn gradient_float_param_slider(
    ui: &mut Ui,
    value: &mut f32,
    spec: FloatParamSpec,
    gradient: SliderGradient,
) -> bool {
    AdjustmentSlider::from_spec(value, spec)
        .gradient(gradient)
        .show(ui)
}

fn gradient_color_at(gradient: SliderGradient, fraction: f32) -> egui::Color32 {
    let t = fraction.clamp(0.0, 1.0);
    match gradient {
        SliderGradient::HueDegrees { start, end } => {
            hsv_color(egui::lerp(start..=end, t), 0.90, 0.92)
        }
        SliderGradient::ChannelHue {
            left,
            center,
            right,
        } => {
            if t <= 0.5 {
                lerp_hue_color(left, center, t * 2.0)
            } else {
                lerp_hue_color(center, right, (t - 0.5) * 2.0)
            }
        }
        SliderGradient::Brightness => {
            if t <= 0.5 {
                lerp_color(
                    crate::ui::theme::BRIGHTNESS_SHADOW,
                    crate::ui::theme::BRIGHTNESS_MID,
                    t * 2.0,
                )
            } else {
                lerp_color(
                    crate::ui::theme::BRIGHTNESS_MID,
                    crate::ui::theme::BRIGHTNESS_HIGHLIGHT,
                    (t - 0.5) * 2.0,
                )
            }
        }
        SliderGradient::Temperature => {
            if t <= 0.5 {
                lerp_color(
                    crate::ui::theme::TEMPERATURE_COOL,
                    crate::ui::theme::TEMPERATURE_NEUTRAL,
                    t * 2.0,
                )
            } else {
                lerp_color(
                    crate::ui::theme::TEMPERATURE_NEUTRAL,
                    crate::ui::theme::TEMPERATURE_WARM,
                    (t - 0.5) * 2.0,
                )
            }
        }
        SliderGradient::Tint => {
            if t <= 0.5 {
                lerp_color(
                    crate::ui::theme::TINT_GREEN,
                    crate::ui::theme::TINT_NEUTRAL,
                    t * 2.0,
                )
            } else {
                lerp_color(
                    crate::ui::theme::TINT_NEUTRAL,
                    crate::ui::theme::TINT_MAGENTA,
                    (t - 0.5) * 2.0,
                )
            }
        }
        SliderGradient::CameraTint { neutral_fraction } => {
            let neutral = neutral_fraction.clamp(0.0, 1.0);
            if t <= neutral {
                let u = if neutral <= f32::EPSILON {
                    1.0
                } else {
                    t / neutral
                };
                lerp_color(
                    crate::ui::theme::TINT_MAGENTA,
                    crate::ui::theme::TINT_NEUTRAL,
                    u,
                )
            } else {
                let span = 1.0 - neutral;
                let u = if span <= f32::EPSILON {
                    1.0
                } else {
                    (t - neutral) / span
                };
                lerp_color(
                    crate::ui::theme::TINT_NEUTRAL,
                    crate::ui::theme::TINT_GREEN,
                    u,
                )
            }
        }
        SliderGradient::Colorfulness => {
            // Saturation and vibrance both operate continuously on chroma across
            // their full bipolar ranges. Keep one stable hue on the track and
            // vary only its colorfulness: the negative end approaches gray,
            // zero keeps a recognizable reference color, and the positive end
            // becomes more saturated. This avoids suggesting that color only
            // starts changing to the right of zero.
            let (hue, reference_saturation, reference_value) =
                rgb_to_hsv(crate::ui::theme::COLORFULNESS_BLUE);
            if t <= 0.5 {
                let u = t * 2.0;
                hsv_color(hue, reference_saturation * u, reference_value)
            } else {
                let u = (t - 0.5) * 2.0;
                hsv_color(
                    hue,
                    egui::lerp(reference_saturation..=1.0, u),
                    reference_value,
                )
            }
        }
        SliderGradient::Saturation(color) => {
            let (hue, saturation, value) = rgb_to_hsv(color);
            let target_saturation = if t <= 0.5 {
                egui::lerp(0.02..=saturation.max(0.35), t * 2.0)
            } else {
                egui::lerp(saturation.max(0.35)..=1.0, (t - 0.5) * 2.0)
            };
            hsv_color(hue, target_saturation, value.max(0.78))
        }
        SliderGradient::Luminance(color) => {
            if t <= 0.5 {
                lerp_color(crate::ui::theme::LUMINANCE_BLACK, color, t * 2.0)
            } else {
                lerp_color(color, crate::ui::theme::LUMINANCE_WHITE, (t - 0.5) * 2.0)
            }
        }
    }
}

fn lerp_color(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    egui::Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

fn hsv_color(hue_degrees: f32, saturation: f32, value: f32) -> egui::Color32 {
    let hue = hue_degrees.rem_euclid(360.0) / 60.0;
    let sector = hue.floor() as u32;
    let blend = hue - sector as f32;
    let saturation = saturation.clamp(0.0, 1.0);
    let value = value.clamp(0.0, 1.0);
    let low = value * (1.0 - saturation);
    let rise = low + (value - low) * blend;
    let fall = value - (value - low) * blend;
    let (red, green, blue) = match sector % 6 {
        0 => (value, rise, low),
        1 => (fall, value, low),
        2 => (low, value, rise),
        3 => (low, fall, value),
        4 => (rise, low, value),
        _ => (value, low, fall),
    };
    egui::Color32::from_rgb(
        (red * 255.0).round() as u8,
        (green * 255.0).round() as u8,
        (blue * 255.0).round() as u8,
    )
}

fn lerp_hue_color(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let (hue_a, saturation_a, value_a) = rgb_to_hsv(a);
    let (hue_b, saturation_b, value_b) = rgb_to_hsv(b);
    let delta = (hue_b - hue_a + 180.0).rem_euclid(360.0) - 180.0;
    hsv_color(
        hue_a + delta * t.clamp(0.0, 1.0),
        egui::lerp(saturation_a..=saturation_b, t.clamp(0.0, 1.0)),
        egui::lerp(value_a..=value_b, t.clamp(0.0, 1.0)),
    )
}

fn rgb_to_hsv(color: egui::Color32) -> (f32, f32, f32) {
    let red = color.r() as f32 / 255.0;
    let green = color.g() as f32 / 255.0;
    let blue = color.b() as f32 / 255.0;
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let delta = max - min;
    let hue = if delta <= f32::EPSILON {
        0.0
    } else if max == red {
        60.0 * ((green - blue) / delta).rem_euclid(6.0)
    } else if max == green {
        60.0 * ((blue - red) / delta + 2.0)
    } else {
        60.0 * ((red - green) / delta + 4.0)
    };
    let saturation = if max <= f32::EPSILON {
        0.0
    } else {
        delta / max
    };
    (hue, saturation, max)
}

#[cfg(test)]
mod tests {
    use super::{gradient_color_at, rgb_to_hsv, AdjustmentSlider, SliderGradient};
    use crate::pipeline::effect_params::FloatParamSpec;
    use eframe::egui::{pos2, Event, Modifiers, PointerButton, RawInput};

    #[test]
    fn spec_slider_double_click_restores_the_spec_default() {
        let spec = FloatParamSpec {
            label: "Strength",
            min: -1.0,
            max: 1.0,
            default: 0.25,
            step: 0.01,
            decimals: 2,
            tooltip: None,
        };
        let ctx = eframe::egui::Context::default();
        let mut value = -0.5_f32;
        let mut time = 0.0;
        let mut show = |events| {
            time += 0.05;
            let input = RawInput {
                screen_rect: Some(eframe::egui::Rect::from_min_size(
                    eframe::egui::Pos2::ZERO,
                    eframe::egui::vec2(400.0, 240.0),
                )),
                time: Some(time),
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                AdjustmentSlider::from_spec(&mut value, spec)
                    .gradient(SliderGradient::Brightness)
                    .show(ui);
            });
        };
        show(Vec::new());
        let label = pos2(10.0, moduwu_design::CONTROL_HEIGHT * 0.5);
        for _ in 0..2 {
            for pressed in [true, false] {
                show(vec![
                    Event::PointerMoved(label),
                    Event::PointerButton {
                        pos: label,
                        button: PointerButton::Primary,
                        pressed,
                        modifiers: Modifiers::NONE,
                    },
                ]);
            }
        }
        assert_eq!(value, 0.25);
    }

    #[test]
    fn colorfulness_gradient_uses_one_hue_and_increases_chroma_across_zero() {
        let quarter = rgb_to_hsv(gradient_color_at(SliderGradient::Colorfulness, 0.25));
        let neutral = rgb_to_hsv(gradient_color_at(SliderGradient::Colorfulness, 0.50));
        let positive = rgb_to_hsv(gradient_color_at(SliderGradient::Colorfulness, 0.75));

        assert!((quarter.0 - neutral.0).abs() < 1.0);
        assert!((neutral.0 - positive.0).abs() < 1.0);
        assert!(quarter.1 < neutral.1);
        assert!(neutral.1 < positive.1);
    }

    #[test]
    fn hue_gradient_wraps_to_the_same_color_at_a_full_turn() {
        let gradient = SliderGradient::HueDegrees {
            start: 0.0,
            end: 360.0,
        };
        assert_eq!(
            gradient_color_at(gradient, 0.0),
            gradient_color_at(gradient, 1.0)
        );
    }

    #[test]
    fn brightness_gradient_is_monotonic_in_luma() {
        let gradient = SliderGradient::Brightness;
        let low = gradient_color_at(gradient, 0.0);
        let mid = gradient_color_at(gradient, 0.5);
        let high = gradient_color_at(gradient, 1.0);
        assert!(low.r() < mid.r() && mid.r() < high.r());
    }

    #[test]
    fn camera_tint_gradient_reverses_local_tint_and_uses_requested_neutral() {
        let neutral_fraction = 0.4;
        let camera = SliderGradient::CameraTint { neutral_fraction };
        let local = SliderGradient::Tint;

        let camera_low = gradient_color_at(camera, 0.0);
        let camera_neutral = gradient_color_at(camera, neutral_fraction);
        let camera_high = gradient_color_at(camera, 1.0);
        let local_low = gradient_color_at(local, 0.0);
        let local_high = gradient_color_at(local, 1.0);

        assert_eq!(camera_low, local_high);
        assert_eq!(camera_high, local_low);
        assert_eq!(camera_neutral, eframe::egui::Color32::from_gray(202));
    }
}
