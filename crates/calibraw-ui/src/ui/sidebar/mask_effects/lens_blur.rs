use super::{
    effect_details, effect_toggle, float_param_angle, float_param_slider, styled_effect_card,
    StyleChoice,
};
use crate::i18n::tr;
use crate::pipeline::{effect_params::defocus, BokehShape, DefocusEffectSettings, MaskEffect};
use crate::ui::components::feathered_range::{
    self, EndEdit, FeatheredRange, RangeEdit, RangeHandle, RangePoints,
};
use crate::ui::theme;
use eframe::egui::{self, pos2, Rect, Ui};

/// The track's tint of each part of the scene, as the depth overlay shows it.
const TRACK_TINT: f32 = 0.35;

/// Lens Blur's focus range on the shared range track: two hard ends in
/// control depth (0 the nearest surface, 1 the farthest), drawn as the depths
/// that stay sharp.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FocusRange {
    /// Focus near and Focus far as stored, in percent.
    near: f32,
    far: f32,
}

impl FocusRange {
    fn of(settings: &DefocusEffectSettings) -> Self {
        Self {
            near: settings.focus_near,
            far: settings.focus_far,
        }
    }

    /// The ends on the track, near first, as the shader reads them.
    fn ends(self) -> [f32; 2] {
        DefocusEffectSettings {
            focus_near: self.near,
            focus_far: self.far,
            ..DefocusEffectSettings::default()
        }
        .focus_range()
    }
}

/// A track place in stored units: percent, at the sliders' precision.
fn stored_percent(place: f32) -> f32 {
    feathered_range::round_to(place * 100.0, defocus::FOCUS_NEAR.decimals as i32)
}

/// The place an end's edit moves it to; a range without fades is only ever
/// moved, never faded.
fn edited_place(edit: EndEdit) -> f32 {
    match edit {
        EndEdit::Moved { full } | EndEdit::Faded { full, .. } | EndEdit::Squeezed { full, .. } => {
            full
        }
    }
}

impl FeatheredRange for FocusRange {
    const HAS_FEATHERS: bool = false;

    fn points(&self) -> RangePoints {
        let [near, far] = self.ends();
        RangePoints {
            fade_in: near,
            full_from: near,
            full_to: far,
            fade_out: far,
        }
    }

    fn set_points(&mut self, edit: RangeEdit) {
        if let Some(start) = edit.start {
            self.near = defocus::FOCUS_NEAR.clamp(stored_percent(edited_place(start)));
        }
        if let Some(end) = edit.end {
            self.far = defocus::FOCUS_FAR.clamp(stored_percent(edited_place(end)));
        }
        // Rounding must not leave Near past Far; the end this step moved
        // yields, so an untouched end stays exact.
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
            RangeHandle::Start | RangeHandle::StartFeather => {
                format!("{} {:.1}", tr(defocus::FOCUS_NEAR.label), self.near)
            }
            RangeHandle::End | RangeHandle::EndFeather => {
                format!("{} {:.1}", tr(defocus::FOCUS_FAR.label), self.far)
            }
        }
    }

    /// 1 at the depths that stay sharp.
    fn weight(&self, t: f32) -> f32 {
        let [near, far] = self.ends();
        f32::from(u8::from((near..=far).contains(&t)))
    }

    fn reset(&mut self) {
        self.near = defocus::FOCUS_NEAR.default;
        self.far = defocus::FOCUS_FAR.default;
    }
}

/// The track's backdrop: near, in focus and far, in the depth overlay's
/// colours.
fn focus_track_background(
    visuals: &egui::Visuals,
    [near, far]: [f32; 2],
) -> impl FnOnce(&egui::Painter, Rect) + use<> {
    let fill = visuals.extreme_bg_color;
    move |painter, track| {
        painter.rect_filled(track, 3.0, fill);
        let x = |t: f32| egui::lerp(track.x_range(), t);
        for (from, to, color) in [
            (0.0, near, theme::FOCUS_NEAR),
            (near, far, theme::FOCUS_IN),
            (far, 1.0, theme::FOCUS_FAR),
        ] {
            if to > from {
                painter.rect_filled(
                    Rect::from_min_max(pos2(x(from), track.top()), pos2(x(to), track.bottom())),
                    0.0,
                    color.gamma_multiply(TRACK_TINT),
                );
            }
        }
    }
}

fn focus_range_adjusting_id() -> egui::Id {
    egui::Id::new("calibraw-lens-blur-focus-range-adjusting")
}

/// The focus range in control depth while one of its handles is held, for
/// the canvas's depth overlay. The desktop sidebar is drawn before the
/// preview, as for [`super::effect_card_open`]; a phone's tool sheet may be
/// drawn after it, so a range noted in the previous pass still counts.
pub(crate) fn focus_range_adjusting(ctx: &egui::Context) -> Option<[f32; 2]> {
    ctx.data(|data| data.get_temp::<(u64, [f32; 2])>(focus_range_adjusting_id()))
        .filter(|(pass, _)| *pass + 1 >= ctx.cumulative_pass_nr())
        .map(|(_, range)| range)
}

fn focus_range(ui: &mut Ui, settings: &mut DefocusEffectSettings) -> bool {
    let mut range = FocusRange::of(settings);
    let before = range;
    let label = tr("Focus range");
    ui.label(label)
        .on_hover_text(tr(defocus::FOCUS_RANGE_HELP));
    let background = focus_track_background(ui.visuals(), range.ends());
    let response = feathered_range::feathered_range_track(ui, &mut range, label, background);
    settings.focus_near = range.near;
    settings.focus_far = range.far;
    if response.dragged() || response.is_pointer_button_down_on() {
        let pass = ui.ctx().cumulative_pass_nr();
        let ends = range.ends();
        ui.ctx()
            .data_mut(|data| data.insert_temp(focus_range_adjusting_id(), (pass, ends)));
    }
    range != before
}

/// The Bokeh shape's choice, with what the shapes look like as its tooltip.
fn bokeh_shape(ui: &mut Ui, shape: &mut BokehShape) -> bool {
    let before = *shape;
    moduwu_design::form_combo_with_help(
        ui,
        &tr("Bokeh shape"),
        "lens-blur-bokeh-shape",
        tr(shape.label()),
        240.0,
        &tr(defocus::BOKEH_SHAPE_HELP),
        |ui| {
            for option in BokehShape::ALL {
                if ui
                    .selectable_label(option == *shape, tr(option.label()))
                    .clicked()
                {
                    *shape = option;
                }
            }
        },
    );
    *shape != before
}

/// The card: the blur, the bokeh, Focus by depth with its focus range, then
/// the Blades aperture under its details.
pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut DefocusEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
    style: Option<&mut StyleChoice>,
) -> bool {
    styled_effect_card(
        ui,
        MaskEffect::Defocus,
        settings,
        enabled,
        remove,
        style,
        |ui, settings| {
            let mut changed =
                float_param_slider(ui, &mut settings.blur_amount, defocus::BLUR_AMOUNT);
            changed |= bokeh_shape(ui, &mut settings.shape);
            changed |= float_param_slider(ui, &mut settings.highlight_boost, defocus::BOOST);
            changed |= effect_toggle(ui, &mut settings.focus_by_depth, defocus::FOCUS_BY_DEPTH);
            if settings.focus_by_depth {
                changed |= focus_range(ui, settings);
            }
            if settings.shape == BokehShape::Blades {
                changed |= effect_details(ui, tr("Aperture details"), |ui| {
                    let mut changed = float_param_slider(ui, &mut settings.blades, defocus::BLADES);
                    changed |= float_param_angle(ui, &mut settings.rotation, defocus::ROTATION);
                    changed
                });
            }
            changed
        },
    )
}
