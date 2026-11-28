use crate::app::HslMixerColor;
use crate::pipeline::{ExposureParams, LocalAdjustments, PointColors, HSL_HUE_LIMIT};
use crate::ui::components::adjustment_slider::{AdjustmentSlider, SliderGradient};
use crate::ui::components::point_color::{point_color, PointColorUiState};
use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Stroke, StrokeKind, Ui};

use crate::i18n::{tr, tr_fmt};
use crate::ui::theme::HSL_CHANNELS as CHANNELS;

const SELECTOR_GAP: f32 = 4.0;
const SELECTOR_HEIGHT: f32 = 30.0;

/// The settings a Color Mixer card edits: the photo's or a mask's.
pub(crate) struct ColorMixerSettings<'a> {
    hue: &'a mut [f32; 8],
    saturation: &'a mut [f32; 8],
    luminance: &'a mut [f32; 8],
    monochrome_mix: &'a mut [f32; 8],
    point_colors: &'a mut PointColors,
}

impl<'a> ColorMixerSettings<'a> {
    pub(crate) fn of_photo(exposure: &'a mut ExposureParams) -> Self {
        Self {
            hue: &mut exposure.hsl_hue,
            saturation: &mut exposure.hsl_saturation,
            luminance: &mut exposure.hsl_luminance,
            monochrome_mix: &mut exposure.monochrome_mix,
            point_colors: &mut exposure.point_colors,
        }
    }

    pub(crate) fn of_mask(adjustments: &'a mut LocalAdjustments) -> Self {
        Self {
            hue: &mut adjustments.hsl_hue,
            saturation: &mut adjustments.hsl_saturation,
            luminance: &mut adjustments.hsl_luminance,
            monochrome_mix: &mut adjustments.monochrome_mix,
            point_colors: &mut adjustments.point_colors,
        }
    }
}

/// The body of a Color Mixer card. While the photo renders black and white
/// (`monochrome`, [`ExposureParams::renders_monochrome`]) it is the mix, which
/// renders in place of the rest; Point Color has no colour to pick in a grey
/// photo. Otherwise it is the Mixer and Point Color tabs (`point_color_tab`).
/// Returns whether a setting changed.
pub(crate) fn color_mixer_body(
    ui: &mut Ui,
    settings: ColorMixerSettings<'_>,
    monochrome: bool,
    selected_color: &mut HslMixerColor,
    point_color_state: &mut PointColorUiState,
    point_color_tab: &mut bool,
) -> bool {
    if monochrome {
        point_color_state.picker_active = false;
        point_color_state.visualize_range = false;
        return monochrome_mix(ui, settings.monochrome_mix);
    }
    ui.horizontal(|ui| {
        ui.selectable_value(point_color_tab, false, tr("Mixer"));
        ui.selectable_value(point_color_tab, true, tr("Point Color"));
    });
    ui.add_space(moduwu_design::SPACE_XS);
    if *point_color_tab {
        point_color(ui, settings.point_colors, point_color_state)
    } else {
        point_color_state.picker_active = false;
        point_color_state.visualize_range = false;
        hsl_mixer(
            ui,
            selected_color,
            settings.hue,
            settings.saturation,
            settings.luminance,
        )
    }
}

fn hsl_mixer(
    ui: &mut Ui,
    selected: &mut HslMixerColor,
    hue: &mut [f32; 8],
    saturation: &mut [f32; 8],
    luminance: &mut [f32; 8],
) -> bool {
    let mut changed = false;

    let (selector_width, selector_gap) = selector_geometry(ui.available_width());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = selector_gap;
        for (index, (name, accent)) in CHANNELS.iter().enumerate() {
            if color_selector_button(
                ui,
                tr(name),
                *accent,
                selected.index() == index,
                selector_width,
            )
            .clicked()
            {
                *selected = HslMixerColor::ALL[index];
            }
        }
    });

    let index = selected.index();
    let (name, accent) = CHANNELS[index];
    let name = tr(name);
    let mut reset_color = false;

    ui.add_space(moduwu_design::SPACE_XS);
    moduwu_design::toolbar_row(ui, |ui| {
        ui.label(RichText::new(tr_fmt("{name} adjustments", &[("name", &name)])).strong());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            reset_color = moduwu_design::ghost_icon_button(
                ui,
                egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                moduwu_design::toolbar_icon_size(),
                tr("Reset Hue, Saturation, and Luminance for this color"),
            )
            .clicked();
        });
    });

    if reset_color
        && (hue[index].abs() > f32::EPSILON
            || saturation[index].abs() > f32::EPSILON
            || luminance[index].abs() > f32::EPSILON)
    {
        hue[index] = 0.0;
        saturation[index] = 0.0;
        luminance[index] = 0.0;
        changed = true;
    }

    ui.push_id(("hsl-color", index), |ui| {
        changed |=
            AdjustmentSlider::new(tr("Hue"), &mut hue[index], -HSL_HUE_LIMIT..=HSL_HUE_LIMIT)
                .decimals(0)
                .step(1.0)
                .accent(accent)
                .gradient(SliderGradient::ChannelHue {
                    left: CHANNELS[(index + CHANNELS.len() - 2) % CHANNELS.len()].1,
                    center: accent,
                    right: CHANNELS[(index + 2) % CHANNELS.len()].1,
                })
                .show(ui);
        changed |= AdjustmentSlider::new(tr("Saturation"), &mut saturation[index], -100.0..=100.0)
            .decimals(0)
            .step(1.0)
            .accent(accent)
            .gradient(SliderGradient::Saturation(accent))
            .show(ui);
        changed |= AdjustmentSlider::new(tr("Luminance"), &mut luminance[index], -100.0..=100.0)
            .decimals(0)
            .step(1.0)
            .accent(accent)
            .gradient(SliderGradient::Luminance(accent))
            .show(ui);
    });

    changed
}

/// The mix of a black-and-white photo ([`ExposureParams::monochrome_mix`]):
/// how much each colour's grey is brightened or darkened, one slider per
/// colour, all at once as Lightroom's B&W mix shows them.
fn monochrome_mix(ui: &mut Ui, mix: &mut [f32; 8]) -> bool {
    let mut changed = false;
    for (index, (name, accent)) in CHANNELS.iter().enumerate() {
        ui.push_id(("monochrome-mix", index), |ui| {
            changed |= AdjustmentSlider::new(tr(name), &mut mix[index], -100.0..=100.0)
                .decimals(0)
                .step(1.0)
                .accent(*accent)
                .gradient(SliderGradient::Luminance(*accent))
                .show(ui);
        });
    }
    changed
}

fn selector_geometry(available_width: f32) -> (f32, f32) {
    let available_width = available_width.max(0.0);
    let gap = SELECTOR_GAP.min(available_width / 14.0);
    let width = (available_width - gap * 7.0) / 8.0;
    (width.max(0.0), gap)
}

fn color_selector_button(
    ui: &mut Ui,
    name: &str,
    accent: Color32,
    selected: bool,
    width: f32,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, SELECTOR_HEIGHT), Sense::click());
    let interaction = moduwu_design::interaction_visuals(ui, &response, selected);

    ui.painter().rect_filled(rect, 4.0, interaction.weak_fill);
    ui.painter()
        .rect_stroke(rect, 4.0, interaction.stroke, StrokeKind::Inside);

    let radius = (rect.width() * 0.25).clamp(4.0, 9.0);
    ui.painter().circle_filled(rect.center(), radius, accent);
    ui.painter().circle_stroke(
        rect.center(),
        radius,
        Stroke::new(1.0, Color32::from_white_alpha(100)),
    );
    response.on_hover_text(tr_fmt("Select the {name} color range", &[("name", &name)]))
}
