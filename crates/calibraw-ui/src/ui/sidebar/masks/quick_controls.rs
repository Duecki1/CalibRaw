//! The portrait layout's quick controls: the selected sub-mask's drawing
//! tool settings in a rail on the photo's right edge, so drawing needs
//! neither the sheet nor its scrolling. A value button opens a vertical
//! slider beside the rail. While the rail shows, the sheet leaves its
//! controls out ([`QuickControl`] in `PropertyEdits`).
//!
//! The rail and slider float over the photo, so they are painted dark and
//! translucent with white marks in every theme, like the other on-photo
//! controls; the selection uses the theme's accent.

use super::*;
use crate::app::{MaskPropertiesControls, MaskPropertyAction};
use crate::pipeline::DEFAULT_COLOR_TOLERANCE;
use eframe::egui::Color32;

/// Space between the rail and the edges of the free photo area.
const MARGIN: f32 = 12.0;
/// Padding between the rail's or the slider's edge and its contents.
const PADDING: f32 = 4.0;
/// Edge of a rail button.
const BUTTON: f32 = 40.0;
/// Space between rail buttons, and the larger one between groups.
const BUTTON_GAP: f32 = 2.0;
const GROUP_GAP: f32 = 10.0;
/// The vertical slider's size, before it shrinks to the free photo area.
const SLIDER_SIZE: egui::Vec2 = egui::vec2(48.0, 232.0);
/// Space between the slider and the rail.
const SLIDER_GAP: f32 = 8.0;
/// Height of the value readout at the top of the slider.
const READOUT: f32 = 28.0;

const SURFACE: Color32 = Color32::from_black_alpha(150);
const MARK: Color32 = Color32::WHITE;

/// A value of a drawing tool, edited with a slider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QuickValue {
    BrushSize,
    BrushFeather,
    ObjectBrushSize,
    RefinementSize,
    RefinementFeather,
    ColorTolerance,
}

/// A control of the selected sub-mask's drawing tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QuickControl {
    /// The brush paints or erases.
    BrushMode,
    /// The subject refinement brush adds or subtracts.
    RefinementMode,
    Value(QuickValue),
}

/// A value's slider settings, shared by the sheet's slider and the rail's.
struct ValueSpec {
    label: &'static str,
    range: std::ops::RangeInclusive<f32>,
    decimals: usize,
    step: f64,
    reset: f32,
}

impl QuickControl {
    /// The controls the sub-mask's tool offers, top to bottom; `tool` is the
    /// properties' tool state.
    pub(super) fn of(component: &MaskComponent, tool: &MaskPropertiesControls) -> &'static [Self] {
        use QuickValue::*;
        match component.geometry {
            MaskGeometry::Brush { .. } => &[
                Self::BrushMode,
                Self::Value(BrushSize),
                Self::Value(BrushFeather),
            ],
            MaskGeometry::Object { .. } => &[Self::Value(ObjectBrushSize)],
            MaskGeometry::ColorRange { .. } => &[Self::Value(ColorTolerance)],
            // Refinement is only active on a Subject or Background sub-mask.
            MaskGeometry::Ai { .. } if tool.refinement_active => &[
                Self::RefinementMode,
                Self::Value(RefinementSize),
                Self::Value(RefinementFeather),
            ],
            _ => &[],
        }
    }

    /// Whether the control can be used: the sub-mask is on and, for the
    /// refinement brush, so is its card.
    fn enabled(self, component: &MaskComponent, tool: &MaskPropertiesControls) -> bool {
        let refinement = matches!(
            self,
            Self::RefinementMode
                | Self::Value(QuickValue::RefinementSize | QuickValue::RefinementFeather)
        );
        component.enabled && (!refinement || tool.refinement_enabled)
    }

    /// The control's rail buttons: a mode has one per mode.
    fn buttons(self) -> &'static [RailButton] {
        use QuickValue::*;
        match self {
            Self::BrushMode => &[
                RailButton::Mode(BrushMode::Paint, false),
                RailButton::Mode(BrushMode::Erase, false),
            ],
            Self::RefinementMode => &[
                RailButton::Mode(BrushMode::Paint, true),
                RailButton::Mode(BrushMode::Erase, true),
            ],
            Self::Value(BrushSize) => &[RailButton::Value(BrushSize)],
            Self::Value(BrushFeather) => &[RailButton::Value(BrushFeather)],
            Self::Value(ObjectBrushSize) => &[RailButton::Value(ObjectBrushSize)],
            Self::Value(RefinementSize) => &[RailButton::Value(RefinementSize)],
            Self::Value(RefinementFeather) => &[RailButton::Value(RefinementFeather)],
            Self::Value(ColorTolerance) => &[RailButton::Value(ColorTolerance)],
        }
    }
}

impl QuickValue {
    fn spec(self) -> ValueSpec {
        match self {
            Self::BrushSize | Self::ObjectBrushSize | Self::RefinementSize => ValueSpec {
                label: tr("Size"),
                range: 0.0025..=0.25,
                decimals: 3,
                step: 0.0025,
                reset: if self == Self::RefinementSize {
                    0.035
                } else {
                    0.055
                },
            },
            Self::BrushFeather | Self::RefinementFeather => ValueSpec {
                label: tr("Feather"),
                range: 0.0..=1.0,
                decimals: 2,
                step: 0.01,
                reset: 0.55,
            },
            Self::ColorTolerance => ValueSpec {
                label: tr("Tolerance"),
                range: 0.005..=1.0,
                decimals: 3,
                step: 0.005,
                reset: DEFAULT_COLOR_TOLERANCE,
            },
        }
    }

    /// The value's slider in the sheet.
    pub(super) fn slider(self, value: &mut f32) -> AdjustmentSlider<'_, f32> {
        let spec = self.spec();
        AdjustmentSlider::new(spec.label, value, spec.range)
            .decimals(spec.decimals)
            .step(spec.step)
            .reset_to(spec.reset)
    }

    fn icon(self) -> &'static str {
        match self {
            Self::BrushSize | Self::ObjectBrushSize | Self::RefinementSize => {
                egui_phosphor::regular::CIRCLE
            }
            Self::BrushFeather | Self::RefinementFeather => egui_phosphor::regular::CIRCLE_DASHED,
            Self::ColorTolerance => egui_phosphor::regular::DROP_HALF,
        }
    }

    /// The current value, from the sub-mask's `geometry` or the `tool` state.
    fn read(self, geometry: &MaskGeometry, tool: &MaskPropertiesControls) -> Option<f32> {
        match (self, geometry) {
            (Self::BrushSize, MaskGeometry::Brush { size, .. }) => Some(*size),
            (Self::BrushFeather, MaskGeometry::Brush { feather, .. }) => Some(*feather),
            (Self::ObjectBrushSize, MaskGeometry::Object { brush_size, .. }) => Some(*brush_size),
            (Self::ColorTolerance, MaskGeometry::ColorRange { tolerance, .. }) => Some(*tolerance),
            (Self::RefinementSize, _) => Some(tool.refinement_size),
            (Self::RefinementFeather, _) => Some(tool.refinement_feather),
            _ => None,
        }
    }

    /// The edit that sets the value.
    pub(super) fn action(self, value: f32) -> MaskPropertyAction {
        match self {
            Self::BrushSize => MaskPropertyAction::SetBrushSize(value),
            Self::BrushFeather => MaskPropertyAction::SetBrushFeather(value),
            Self::ObjectBrushSize => MaskPropertyAction::SetObjectBrushSize(value),
            Self::RefinementSize => MaskPropertyAction::SetRefinementSize(value),
            Self::RefinementFeather => MaskPropertyAction::SetRefinementFeather(value),
            Self::ColorTolerance => MaskPropertyAction::SetColorTolerance(value),
        }
    }
}

/// One button of the rail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RailButton {
    /// Selects a brush mode; `true` for the subject refinement brush.
    Mode(BrushMode, bool),
    /// Opens or closes the value's slider.
    Value(QuickValue),
}

impl RailButton {
    fn icon(self) -> &'static str {
        match self {
            Self::Mode(BrushMode::Paint, false) => egui_phosphor::regular::PAINT_BRUSH,
            Self::Mode(BrushMode::Erase, false) => egui_phosphor::regular::ERASER,
            Self::Mode(BrushMode::Paint, true) => egui_phosphor::regular::PLUS_CIRCLE,
            Self::Mode(BrushMode::Erase, true) => egui_phosphor::regular::MINUS_CIRCLE,
            Self::Value(value) => value.icon(),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Mode(BrushMode::Paint, false) => tr("Brush"),
            Self::Mode(BrushMode::Erase, false) => tr("Eraser"),
            Self::Mode(BrushMode::Paint, true) => tr("Add subject"),
            Self::Mode(BrushMode::Erase, true) => tr("Subtract subject"),
            Self::Value(value) => value.spec().label,
        }
    }
}

/// The value whose slider is open, kept between frames.
fn open_value_id() -> egui::Id {
    egui::Id::new("mask-quick-controls-open")
}

/// The unsnapped value a slider drag has reached, so steps finer than one
/// frame's movement add up.
fn drag_value_id(slider: egui::Id) -> egui::Id {
    slider.with("unsnapped")
}

impl Sidebar {
    /// The controls the rail shows for `component`: its tool's, when the
    /// free photo area has room for them, else none.
    pub(super) fn quick_controls_on_photo(
        ctx: &egui::Context,
        component: &MaskComponent,
        tool: &MaskPropertiesControls,
    ) -> &'static [QuickControl] {
        let controls = QuickControl::of(component, tool);
        let fits = Self::photo_overlay_region(ctx)
            .and_then(|region| rail_rect(region, controls))
            .is_some();
        if fits {
            controls
        } else {
            &[]
        }
    }

    /// Shows `controls` in the rail at the photo's right edge, and the open
    /// value's slider beside it, and returns the edits made, in order.
    pub(super) fn show_quick_controls(
        ctx: &egui::Context,
        controls: &[QuickControl],
        component: &MaskComponent,
        tool: &MaskPropertiesControls,
    ) -> Vec<MaskPropertyAction> {
        let mut actions = Vec::new();
        let Some((region, rail)) = Self::photo_overlay_region(ctx)
            .and_then(|region| rail_rect(region, controls).map(|rail| (region, rail)))
        else {
            return actions;
        };
        let mut open = ctx
            .data(|data| data.get_temp::<QuickValue>(open_value_id()))
            .filter(|value| controls.contains(&QuickControl::Value(*value)));
        // A press anywhere but the rail or the open slider closes the slider,
        // so painting or using the sheet puts it away.
        let pressed_elsewhere = ctx.input(|input| {
            input.pointer.any_pressed()
                && input.pointer.press_origin().is_some_and(|origin| {
                    !rail.contains(origin) && !slider_rect(region, rail).contains(origin)
                })
        });
        if pressed_elsewhere {
            open = None;
        }

        egui::Area::new(egui::Id::new("mask-quick-controls"))
            .order(egui::Order::Middle)
            .fixed_pos(rail.min)
            .movable(false)
            .constrain(false)
            .show(ctx, |ui| {
                ui.allocate_exact_size(rail.size(), egui::Sense::hover());
                ui.painter().rect_filled(rail, rail.width() * 0.5, SURFACE);
                let mut top = rail.top() + PADDING;
                let mut index = 0;
                for (group, &control) in controls.iter().enumerate() {
                    if group > 0 {
                        top += GROUP_GAP - BUTTON_GAP;
                    }
                    let enabled = control.enabled(component, tool);
                    for &button in control.buttons() {
                        let rect = egui::Rect::from_min_size(
                            egui::pos2(rail.left() + PADDING, top),
                            egui::Vec2::splat(BUTTON),
                        );
                        top += BUTTON + BUTTON_GAP;
                        index += 1;
                        let selected = match button {
                            RailButton::Mode(mode, _) => tool.brush_mode == mode,
                            RailButton::Value(value) => open == Some(value),
                        };
                        let id = egui::Id::new(("mask-quick-control", index));
                        if !rail_button(ui, rect, id, button, selected, enabled).clicked() {
                            continue;
                        }
                        match button {
                            RailButton::Mode(mode, _) => {
                                if tool.brush_mode != mode {
                                    actions.push(MaskPropertyAction::SetBrushMode(mode));
                                }
                            }
                            RailButton::Value(value) => {
                                open = (open != Some(value)).then_some(value);
                            }
                        }
                    }
                }
            });

        if let Some(value) = open {
            let enabled = QuickControl::Value(value).enabled(component, tool);
            if let Some(current) = value.read(&component.geometry, tool) {
                let slider = slider_rect(region, rail);
                egui::Area::new(egui::Id::new("mask-quick-controls-slider"))
                    .order(egui::Order::Middle)
                    .fixed_pos(slider.min)
                    .movable(false)
                    .constrain(false)
                    .show(ctx, |ui| {
                        let mut edited = current;
                        if vertical_slider(ui, slider, &value.spec(), &mut edited, enabled) {
                            actions.push(value.action(edited));
                        }
                    });
            }
        }
        ctx.data_mut(|data| match open {
            Some(value) => {
                data.insert_temp(open_value_id(), value);
            }
            None => data.remove::<QuickValue>(open_value_id()),
        });
        actions
    }
}

/// The rail for `controls` at the right edge of `region`, centred in its
/// height, or `None` when it and an open slider do not fit.
fn rail_rect(region: egui::Rect, controls: &[QuickControl]) -> Option<egui::Rect> {
    if controls.is_empty() {
        return None;
    }
    let buttons: usize = controls.iter().map(|control| control.buttons().len()).sum();
    let height = PADDING * 2.0
        + BUTTON * buttons as f32
        + BUTTON_GAP * (buttons - 1) as f32
        + (GROUP_GAP - BUTTON_GAP) * (controls.len() - 1) as f32;
    let size = egui::vec2(BUTTON + PADDING * 2.0, height);
    let room = region.shrink(MARGIN);
    let fits = room.height() >= size.y.max(SLIDER_SIZE.y * 0.5)
        && room.width() >= size.x + SLIDER_GAP + SLIDER_SIZE.x;
    fits.then(|| {
        egui::Rect::from_min_size(
            egui::pos2(room.right() - size.x, room.center().y - size.y * 0.5),
            size,
        )
    })
}

/// The open slider beside `rail`, centred on it and kept inside `region`.
fn slider_rect(region: egui::Rect, rail: egui::Rect) -> egui::Rect {
    let room = region.shrink(MARGIN);
    let height = SLIDER_SIZE.y.min(room.height());
    let top = (rail.center().y - height * 0.5).clamp(room.top(), room.bottom() - height);
    egui::Rect::from_min_size(
        egui::pos2(rail.left() - SLIDER_GAP - SLIDER_SIZE.x, top),
        egui::vec2(SLIDER_SIZE.x, height),
    )
}

/// A round rail button at `rect`. A value whose slider is open has the
/// accent behind it; the active brush mode a light disc, the other mode a
/// dimmed mark, so the mode and the open slider read apart.
fn rail_button(
    ui: &mut Ui,
    rect: egui::Rect,
    id: egui::Id,
    button: RailButton,
    selected: bool,
    enabled: bool,
) -> egui::Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let label = button.label();
    let response = ui.interact(rect, id, sense).on_hover_text(label);
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, selected, label)
    });
    let painter = ui.painter();
    let center = rect.center();
    let is_mode = matches!(button, RailButton::Mode(..));
    let fill = match (selected, is_mode) {
        (true, false) => Some(ui.visuals().selection.bg_fill),
        (true, true) => Some(Color32::from_white_alpha(48)),
        (false, _) if response.hovered() || response.has_focus() => {
            Some(Color32::from_white_alpha(24))
        }
        (false, _) => None,
    };
    if let Some(fill) = fill {
        painter.circle_filled(center, BUTTON * 0.5, fill);
    }
    let mark = if !enabled {
        MARK.gamma_multiply(0.35)
    } else if is_mode && !selected {
        MARK.gamma_multiply(0.6)
    } else {
        MARK
    };
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        button.icon(),
        egui::FontId::proportional(20.0),
        mark,
    );
    response
}

/// A vertical slider filling `rect`, its value above the track. A drag moves
/// the value by the distance dragged, so a finger need not cover the thumb;
/// a tap sets it where tapped, a double tap resets it, and the arrow keys
/// step it while focused. Returns whether `value` changed.
fn vertical_slider(
    ui: &mut Ui,
    rect: egui::Rect,
    spec: &ValueSpec,
    value: &mut f32,
    enabled: bool,
) -> bool {
    let id = egui::Id::new(("mask-quick-controls-slider", spec.label));
    let sense = if enabled {
        egui::Sense::click_and_drag()
    } else {
        egui::Sense::hover()
    };
    let response = ui.interact(rect, id, sense);
    response.widget_info(|| egui::WidgetInfo::slider(enabled, f64::from(*value), spec.label));
    let (min, max) = (*spec.range.start(), *spec.range.end());
    let track = egui::Rect::from_min_max(
        egui::pos2(rect.center().x, rect.top() + READOUT + PADDING * 2.0),
        egui::pos2(rect.center().x, rect.bottom() - PADDING * 4.0),
    );
    let fraction_at = |y: f32| ((track.bottom() - y) / track.height().max(1.0)).clamp(0.0, 1.0);
    let snap = |raw: f32| {
        let step = spec.step as f32;
        (min + ((raw - min) / step).round() * step).clamp(min, max)
    };

    let before = *value;
    if moduwu_design::double_tapped(&response) {
        *value = spec.reset;
    } else if response.drag_started() {
        ui.data_mut(|data| data.insert_temp(drag_value_id(id), *value));
    } else if response.dragged() {
        let raw = ui
            .data(|data| data.get_temp::<f32>(drag_value_id(id)))
            .unwrap_or(*value)
            - response.drag_delta().y / track.height().max(1.0) * (max - min);
        let raw = raw.clamp(min, max);
        ui.data_mut(|data| data.insert_temp(drag_value_id(id), raw));
        *value = snap(raw);
    } else if response.clicked() {
        if let Some(pointer) = response.interact_pointer_pos() {
            *value = snap(egui::lerp(min..=max, fraction_at(pointer.y)));
        }
    }
    if response.drag_stopped() {
        ui.data_mut(|data| data.remove::<f32>(drag_value_id(id)));
    }
    if response.has_focus() {
        let steps = ui.input(|input| {
            i32::from(input.key_pressed(egui::Key::ArrowUp))
                - i32::from(input.key_pressed(egui::Key::ArrowDown))
        });
        if steps != 0 {
            *value = snap(*value + steps as f32 * spec.step as f32);
        }
    }

    let painter = ui.painter();
    painter.rect_filled(rect, rect.width() * 0.5, SURFACE);
    let mark = if enabled {
        MARK
    } else {
        MARK.gamma_multiply(0.35)
    };
    painter.text(
        egui::pos2(rect.center().x, rect.top() + PADDING + READOUT * 0.5),
        egui::Align2::CENTER_CENTER,
        format!("{:.*}", spec.decimals, value),
        egui::FontId::proportional(12.0),
        mark,
    );
    let fraction = (*value - min) / (max - min).max(f32::EPSILON);
    let thumb = egui::pos2(
        track.center().x,
        egui::lerp(track.bottom()..=track.top(), fraction),
    );
    painter.line_segment(
        [track.center_top(), track.center_bottom()],
        egui::Stroke::new(4.0, Color32::from_white_alpha(60)),
    );
    painter.line_segment(
        [thumb, track.center_bottom()],
        egui::Stroke::new(4.0, ui.visuals().selection.bg_fill),
    );
    painter.circle_filled(thumb, 10.0, Color32::from_black_alpha(70));
    painter.circle_filled(thumb, 9.0, mark);
    if response.has_focus() {
        painter.rect_stroke(
            rect,
            rect.width() * 0.5,
            ui.visuals().selection.stroke,
            egui::StrokeKind::Inside,
        );
    }
    *value != before
}
