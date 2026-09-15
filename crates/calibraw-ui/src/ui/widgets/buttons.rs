#[cfg(any(target_os = "android", test))]
use eframe::egui::Vec2;
use eframe::egui::{self, Color32, InnerResponse, Response, RichText, Stroke, Ui};

use super::super::theme::{CARD_RADIUS, CONTROL_HEIGHT, SPACE_SM};
#[cfg(any(target_os = "android", test))]
use super::super::theme::{FLOATING_ACTION_EDGE, FLOATING_ACTION_MARGIN};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InteractionVisualState {
    Disabled,
    Active,
    Selected,
    Focused,
    Hovered,
    Inactive,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct InteractionVisuals {
    pub(crate) state: InteractionVisualState,
    pub(crate) fill: Color32,
    pub(crate) weak_fill: Color32,
    pub(crate) stroke: Stroke,
    pub(crate) foreground: Color32,
}

pub(crate) const fn interaction_visual_state(
    enabled: bool,
    selected: bool,
    active: bool,
    hovered: bool,
    focused: bool,
) -> InteractionVisualState {
    if !enabled {
        InteractionVisualState::Disabled
    } else if active {
        InteractionVisualState::Active
    } else if selected {
        InteractionVisualState::Selected
    } else if focused {
        InteractionVisualState::Focused
    } else if hovered {
        InteractionVisualState::Hovered
    } else {
        InteractionVisualState::Inactive
    }
}

pub(crate) fn interaction_visuals_for_flags(
    ui: &Ui,
    enabled: bool,
    selected: bool,
    active: bool,
    hovered: bool,
    focused: bool,
) -> InteractionVisuals {
    let state = interaction_visual_state(enabled, selected, active, hovered, focused);
    let visuals = ui.visuals();
    if state == InteractionVisualState::Selected {
        return InteractionVisuals {
            state,
            fill: visuals.selection.bg_fill,
            weak_fill: visuals.selection.bg_fill,
            stroke: visuals.selection.stroke,
            foreground: visuals.selection.stroke.color,
        };
    }

    let widget = match state {
        InteractionVisualState::Disabled => &visuals.widgets.noninteractive,
        InteractionVisualState::Active => &visuals.widgets.active,
        InteractionVisualState::Focused | InteractionVisualState::Hovered => {
            &visuals.widgets.hovered
        }
        InteractionVisualState::Inactive => &visuals.widgets.inactive,
        InteractionVisualState::Selected => unreachable!(),
    };
    InteractionVisuals {
        state,
        fill: widget.bg_fill,
        weak_fill: widget.weak_bg_fill,
        stroke: widget.bg_stroke,
        foreground: widget.fg_stroke.color,
    }
}

pub(crate) fn interaction_visuals(
    ui: &Ui,
    response: &Response,
    selected: bool,
) -> InteractionVisuals {
    interaction_visuals_for_flags(
        ui,
        response.enabled(),
        selected,
        response.is_pointer_button_down_on(),
        response.hovered() || response.highlighted(),
        response.has_focus(),
    )
}

pub(crate) fn full_width_button(ui: &mut Ui, label: impl Into<egui::WidgetText>) -> Response {
    ui.add_sized(
        [ui.available_width().max(1.0), CONTROL_HEIGHT],
        egui::Button::new(label.into()),
    )
}

#[cfg(not(target_os = "android"))]
pub(crate) fn tab_button(ui: &mut Ui, label: &str, selected: bool, width: f32) -> Response {
    segmented_button(ui, RichText::new(label).strong(), selected, width)
}

pub(crate) fn segmented_button(
    ui: &mut Ui,
    label: impl Into<egui::WidgetText>,
    selected: bool,
    width: f32,
) -> Response {
    ui.add_sized(
        [width, CONTROL_HEIGHT],
        egui::Button::new(label.into())
            .selected(selected)
            .frame(true)
            .truncate()
            .corner_radius(CARD_RADIUS),
    )
}

pub(crate) fn toolbar_button(
    ui: &mut Ui,
    label: impl Into<egui::WidgetText>,
    width: f32,
) -> Response {
    ui.add_sized([width, CONTROL_HEIGHT], egui::Button::new(label.into()))
}

fn primary_button_impl(
    ui: &mut Ui,
    label: impl Into<egui::WidgetText>,
    width: Option<f32>,
) -> Response {
    let visuals = &ui.visuals().widgets.active;
    let button = egui::Button::new(label.into().color(Color32::WHITE))
        .fill(visuals.weak_bg_fill)
        .stroke(visuals.bg_stroke)
        .corner_radius(CARD_RADIUS);
    if let Some(width) = width {
        ui.add_sized([width, CONTROL_HEIGHT], button)
    } else {
        ui.add(button.min_size(egui::vec2(0.0, CONTROL_HEIGHT)))
    }
}

pub(crate) fn primary_button(
    ui: &mut Ui,
    label: impl Into<egui::WidgetText>,
    width: f32,
) -> Response {
    primary_button_impl(ui, label, Some(width))
}

pub(crate) fn primary_action_button(ui: &mut Ui, label: impl Into<egui::WidgetText>) -> Response {
    primary_button_impl(ui, label, None)
}

pub(crate) fn secondary_button(ui: &mut Ui, label: impl Into<egui::WidgetText>) -> Response {
    ui.add(
        egui::Button::new(label.into())
            .corner_radius(CARD_RADIUS)
            .min_size(egui::vec2(0.0, CONTROL_HEIGHT)),
    )
}

pub(crate) fn secondary_button_enabled(
    ui: &mut Ui,
    enabled: bool,
    label: impl Into<egui::WidgetText>,
) -> Response {
    ui.add_enabled_ui(enabled, |ui| secondary_button(ui, label))
        .inner
}

pub(crate) fn destructive_button(ui: &mut Ui, label: impl Into<egui::WidgetText>) -> Response {
    let color = ui.visuals().error_fg_color;
    ui.add(
        egui::Button::new(label.into().color(color))
            .corner_radius(CARD_RADIUS)
            .min_size(egui::vec2(0.0, CONTROL_HEIGHT)),
    )
}

pub(crate) fn toggle_button(
    ui: &mut Ui,
    label: impl Into<egui::WidgetText>,
    selected: bool,
) -> Response {
    ui.add(
        egui::Button::new(label.into())
            .selected(selected)
            .frame(true)
            .corner_radius(CARD_RADIUS),
    )
}

pub(crate) fn navigation_row(
    ui: &mut Ui,
    label: impl Into<egui::WidgetText>,
    selected: bool,
    sense: egui::Sense,
) -> Response {
    ui.add_sized(
        [ui.available_width().max(1.0), CONTROL_HEIGHT],
        egui::Button::selectable(selected, ())
            .left_text(label)
            .truncate()
            .sense(sense),
    )
}

pub(crate) fn action_row<R>(
    ui: &mut Ui,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().interact_size.y = CONTROL_HEIGHT;
        ui.spacing_mut().item_spacing = egui::vec2(SPACE_SM, SPACE_SM);
        add_contents(ui)
    })
}

#[cfg(any(target_os = "android", test))]
pub(crate) fn floating_action_rect(bounds: egui::Rect) -> egui::Rect {
    let size = Vec2::splat(FLOATING_ACTION_EDGE);
    let inset = Vec2::splat(FLOATING_ACTION_MARGIN);
    egui::Rect::from_min_size(bounds.right_bottom() - inset - size, size)
}

#[cfg(target_os = "android")]
pub fn floating_action_button(
    ui: &mut Ui,
    rect: egui::Rect,
    glyph: &str,
    tooltip: &str,
) -> Response {
    let active = &ui.visuals().widgets.active;
    let fill = active.weak_bg_fill;
    let stroke = active.bg_stroke;
    let corner_radius = active.corner_radius;
    let icon_color = active.fg_stroke.color;
    ui.put(
        rect,
        egui::Button::new(
            RichText::new(glyph)
                .size(FLOATING_ACTION_EDGE * 0.42)
                .color(icon_color),
        )
        .min_size(rect.size())
        .corner_radius(corner_radius)
        .fill(fill)
        .stroke(stroke),
    )
    .on_hover_text(tooltip)
}
