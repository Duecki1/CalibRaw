use eframe::egui::{
    self, Align, Color32, Frame, InnerResponse, Layout, Margin, Response, RichText, Stroke, Ui,
    Vec2,
};
use serde::{Deserialize, Serialize};

#[cfg(test)]
use super::widgets::buttons::{destructive_button, interaction_visual_state};
#[cfg(test)]
use super::dialogs::take_initial_focus_request;
pub(crate) use super::dialogs::{
    dialog_button_row, dialog_confirmation_buttons, dialog_keyboard_action, dialog_window,
    request_initial_focus, DialogAction, DialogKeyboard, DIALOG_MARGIN, DIALOG_TEXT_FIELD_WIDTH,
    DIALOG_WIDTH_DEFAULT, DIALOG_WIDTH_FORM, DIALOG_WIDTH_LARGE, DIALOG_WIDTH_NARROW,
    DIALOG_WIDTH_WIDE,
};
#[cfg(test)]
use super::responsive::compact_portrait_for_platform;
use super::responsive::content_margin;
pub(crate) use super::responsive::{card_gap, is_compact_portrait};
#[cfg(target_os = "android")]
pub use super::widgets::buttons::floating_action_button;
#[cfg(any(target_os = "android", test))]
pub(crate) use super::widgets::buttons::floating_action_rect;
#[cfg(not(target_os = "android"))]
pub(crate) use super::widgets::buttons::tab_button;
pub(crate) use super::widgets::buttons::{
    action_row, full_width_button, interaction_visuals, interaction_visuals_for_flags,
    navigation_row, primary_action_button, primary_button, secondary_button,
    secondary_button_enabled, segmented_button, toggle_button, toolbar_button,
    InteractionVisualState,
};
pub(crate) use super::widgets::forms::{
    checkbox_with_help, combo_box, form_combo, form_combo_with_help, form_row, heading_with_help,
    property_row, responsive_combo_box, singleline_text_edit, strong_with_help,
};
pub(crate) use super::widgets::menus::{
    context_menu, context_menu_item, destructive_menu_item, dropdown_menu, dropdown_submenu,
    menu_item,
};

const DESKTOP_CONTROL_HEIGHT: f32 = 32.0;
const ANDROID_CONTROL_HEIGHT: f32 = 40.0;
pub(crate) const CONTROL_HEIGHT: f32 = platform_control_height(cfg!(target_os = "android"));
pub(crate) const TOOLBAR_HEIGHT: f32 = if cfg!(target_os = "android") {
    ANDROID_CONTROL_HEIGHT
} else {
    DESKTOP_CONTROL_HEIGHT
};
pub(crate) const TOOLBAR_ICON_EDGE: f32 = CONTROL_HEIGHT;
#[cfg(not(target_os = "android"))]
pub(crate) const TOOL_RAIL_ICON_EDGE: f32 = 40.0;
pub(crate) const SPACE_XXS: f32 = 2.0;
pub(crate) const SPACE_XS: f32 = 4.0;
pub(crate) const SPACE_SM: f32 = 8.0;
pub(crate) const SPACE_MD: f32 = 12.0;
pub(crate) const SPACE_LG: f32 = 16.0;
pub(crate) const CARD_GAP: f32 = SPACE_SM;
pub(crate) const CONTENT_MARGIN: i8 = 12;
pub(crate) const CARD_RADIUS: f32 = 8.0;
pub(crate) const HELP_BUTTON_EDGE: f32 = if cfg!(target_os = "android") {
    CONTROL_HEIGHT
} else {
    28.0
};
#[cfg(test)]
pub(crate) const PANEL_TITLE_HEIGHT: f32 = 40.0;
pub(crate) const PANEL_TITLE_TEXT_SIZE: f32 = 16.0;
#[cfg(any(target_os = "android", test))]
pub(crate) const FLOATING_ACTION_EDGE: f32 =
    platform_floating_action_edge(cfg!(target_os = "android"));
#[cfg(any(target_os = "android", test))]
pub(crate) const FLOATING_ACTION_MARGIN: f32 = SPACE_MD;

pub(crate) const CANVAS_BACKDROP: Color32 = Color32::from_rgb(13, 15, 18);
pub(crate) const STATUS_WARNING: Color32 = Color32::from_rgb(244, 142, 48);
pub(crate) const MASK_ADD: Color32 = Color32::from_rgb(78, 163, 255);
pub(crate) const MASK_SUBTRACT: Color32 = Color32::from_rgb(255, 105, 105);
pub(crate) const DROP_TARGET: Color32 = Color32::from_rgb(225, 62, 62);
pub(crate) fn inpaint_stroke_highlight() -> Color32 {
    Color32::from_rgba_unmultiplied(255, 96, 78, 62)
}

pub(crate) fn inpaint_stroke_active() -> Color32 {
    Color32::from_rgba_unmultiplied(255, 120, 84, 84)
}
pub(crate) const CHANNEL_RED: Color32 = Color32::from_rgb(238, 84, 84);
pub(crate) const CHANNEL_GREEN: Color32 = Color32::from_rgb(92, 210, 116);
pub(crate) const CHANNEL_BLUE: Color32 = Color32::from_rgb(88, 150, 245);

pub(crate) const MASK_COMPONENT_COLORS: [Color32; 8] = [
    MASK_ADD,
    Color32::from_rgb(255, 116, 102),
    Color32::from_rgb(83, 211, 146),
    Color32::from_rgb(242, 192, 75),
    Color32::from_rgb(183, 124, 255),
    Color32::from_rgb(63, 207, 220),
    Color32::from_rgb(255, 133, 196),
    Color32::from_rgb(180, 205, 88),
];

pub(crate) const HSL_CHANNELS: [(&str, Color32); 8] = [
    ("Red", Color32::from_rgb(232, 76, 82)),
    ("Orange", Color32::from_rgb(238, 137, 48)),
    ("Yellow", Color32::from_rgb(224, 193, 57)),
    ("Green", Color32::from_rgb(75, 184, 101)),
    ("Aqua", Color32::from_rgb(52, 184, 184)),
    ("Blue", Color32::from_rgb(73, 130, 232)),
    ("Purple", Color32::from_rgb(153, 94, 218)),
    ("Magenta", Color32::from_rgb(219, 79, 163)),
];

pub(crate) const BRIGHTNESS_SHADOW: Color32 = Color32::from_gray(18);
pub(crate) const BRIGHTNESS_MID: Color32 = Color32::from_gray(118);
pub(crate) const BRIGHTNESS_HIGHLIGHT: Color32 = Color32::from_gray(245);
pub(crate) const TEMPERATURE_COOL: Color32 = Color32::from_rgb(72, 128, 235);
pub(crate) const TEMPERATURE_NEUTRAL: Color32 = Color32::from_gray(208);
pub(crate) const TEMPERATURE_WARM: Color32 = Color32::from_rgb(244, 157, 62);
pub(crate) const TINT_GREEN: Color32 = Color32::from_rgb(76, 181, 112);
pub(crate) const TINT_NEUTRAL: Color32 = Color32::from_gray(202);
pub(crate) const TINT_MAGENTA: Color32 = Color32::from_rgb(222, 84, 174);
pub(crate) const COLORFULNESS_SHADOW: Color32 = Color32::from_gray(92);
pub(crate) const COLORFULNESS_MID: Color32 = Color32::from_gray(178);
pub(crate) const LUMINANCE_BLACK: Color32 = Color32::from_rgb(10, 10, 10);
pub(crate) const LUMINANCE_WHITE: Color32 = Color32::from_rgb(246, 246, 246);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum UiDesign {
    #[default]
    #[serde(rename = "midnight_pink")]
    ObsidianBlue,
    #[serde(rename = "graphite_mint")]
    ObsidianRed,
    Porcelain,
    DaylightBlue,
}

impl UiDesign {
    pub(crate) const ALL: [Self; 4] = [
        Self::ObsidianBlue,
        Self::ObsidianRed,
        Self::Porcelain,
        Self::DaylightBlue,
    ];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::ObsidianBlue => "Obsidian Blue · Dark",
            Self::ObsidianRed => "Obsidian Red · Dark",
            Self::Porcelain => "Porcelain · Light",
            Self::DaylightBlue => "Daylight · Light",
        }
    }

    pub(crate) const fn description(self) -> &'static str {
        match self {
            Self::ObsidianBlue => "Deep neutral surfaces with a restrained blue accent.",
            Self::ObsidianRed => "Warm near-black surfaces with a restrained ruby accent.",
            Self::Porcelain => "Warm paper-like surfaces with a restrained coral accent.",
            Self::DaylightBlue => "Clean cool surfaces with a focused blue accent.",
        }
    }

    pub(crate) const fn is_dark(self) -> bool {
        matches!(self, Self::ObsidianBlue | Self::ObsidianRed)
    }

    const fn palette(self) -> ThemePalette {
        match self {
            Self::ObsidianBlue => ThemePalette {
                accent: Color32::from_rgb(79, 132, 185),
                accent_bright: Color32::from_rgb(126, 170, 213),
                hyperlink: Color32::from_rgb(111, 162, 210),
                border: Color32::from_rgb(48, 52, 60),
                panel: Color32::from_rgb(24, 26, 31),
                window: Color32::from_rgb(18, 20, 24),
                faint: Color32::from_rgb(31, 34, 40),
                extreme: Color32::from_rgb(12, 14, 17),
                inactive: Color32::from_rgb(34, 37, 43),
                inactive_stroke: Color32::from_rgb(58, 63, 72),
                hovered: Color32::from_rgb(43, 47, 55),
                hovered_stroke: Color32::from_rgb(79, 86, 98),
                open: Color32::from_rgb(39, 43, 50),
                open_stroke: Color32::from_rgb(69, 76, 88),
            },
            Self::ObsidianRed => ThemePalette {
                accent: Color32::from_rgb(166, 70, 86),
                accent_bright: Color32::from_rgb(211, 111, 125),
                hyperlink: Color32::from_rgb(222, 125, 137),
                border: Color32::from_rgb(55, 45, 49),
                panel: Color32::from_rgb(25, 22, 24),
                window: Color32::from_rgb(18, 16, 18),
                faint: Color32::from_rgb(34, 29, 31),
                extreme: Color32::from_rgb(11, 10, 11),
                inactive: Color32::from_rgb(39, 33, 35),
                inactive_stroke: Color32::from_rgb(67, 52, 56),
                hovered: Color32::from_rgb(49, 40, 43),
                hovered_stroke: Color32::from_rgb(99, 69, 76),
                open: Color32::from_rgb(44, 36, 39),
                open_stroke: Color32::from_rgb(88, 61, 67),
            },
            Self::Porcelain => ThemePalette {
                accent: Color32::from_rgb(232, 132, 169),
                accent_bright: Color32::from_rgb(242, 166, 194),
                hyperlink: Color32::from_rgb(173, 36, 92),
                border: Color32::from_rgb(205, 195, 199),
                panel: Color32::from_rgb(247, 243, 244),
                window: Color32::from_rgb(255, 251, 252),
                faint: Color32::from_rgb(241, 235, 237),
                extreme: Color32::from_rgb(225, 216, 219),
                inactive: Color32::from_rgb(237, 229, 232),
                inactive_stroke: Color32::from_rgb(196, 183, 188),
                hovered: Color32::from_rgb(230, 218, 223),
                hovered_stroke: Color32::from_rgb(176, 157, 165),
                open: Color32::from_rgb(226, 213, 218),
                open_stroke: Color32::from_rgb(168, 149, 157),
            },
            Self::DaylightBlue => ThemePalette {
                accent: Color32::from_rgb(116, 170, 242),
                accent_bright: Color32::from_rgb(151, 195, 250),
                hyperlink: Color32::from_rgb(28, 91, 193),
                border: Color32::from_rgb(190, 199, 211),
                panel: Color32::from_rgb(243, 247, 252),
                window: Color32::from_rgb(251, 253, 255),
                faint: Color32::from_rgb(235, 241, 248),
                extreme: Color32::from_rgb(216, 225, 236),
                inactive: Color32::from_rgb(229, 236, 245),
                inactive_stroke: Color32::from_rgb(178, 190, 205),
                hovered: Color32::from_rgb(217, 227, 239),
                hovered_stroke: Color32::from_rgb(151, 170, 193),
                open: Color32::from_rgb(211, 223, 237),
                open_stroke: Color32::from_rgb(143, 163, 187),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PreviewBackdrop {
    Black,
    #[default]
    DarkGrey,
    LightGrey,
    White,
    MatchPhoto,
}

impl PreviewBackdrop {
    pub(crate) const ALL: [Self; 5] = [
        Self::Black,
        Self::DarkGrey,
        Self::MatchPhoto,
        Self::LightGrey,
        Self::White,
    ];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Black => "Black",
            Self::DarkGrey => "Dark grey",
            Self::LightGrey => "Light grey",
            Self::White => "White",
            Self::MatchPhoto => "Match photo",
        }
    }

    pub(crate) const fn color(self, adaptive: Color32) -> Color32 {
        match self {
            Self::Black => Color32::BLACK,
            Self::DarkGrey => CANVAS_BACKDROP,
            Self::LightGrey => Color32::from_gray(168),
            Self::White => Color32::WHITE,
            Self::MatchPhoto => adaptive,
        }
    }
}

pub(crate) fn adaptive_backdrop_from_rgba(rgba: &[u8]) -> Color32 {
    let pixel_count = rgba.len() / 4;
    if pixel_count == 0 {
        return CANVAS_BACKDROP;
    }

    let sample_stride = (pixel_count / 4096).max(1);
    let mut sums = [0_u64; 3];
    let mut weight = 0_u64;
    for pixel in rgba.chunks_exact(4).step_by(sample_stride) {
        let alpha = u64::from(pixel[3]);
        if alpha == 0 {
            continue;
        }
        for channel in 0..3 {
            sums[channel] += u64::from(pixel[channel]) * alpha;
        }
        weight += alpha;
    }
    if weight == 0 {
        return CANVAS_BACKDROP;
    }

    let average = sums.map(|sum| sum as f32 / weight as f32);
    let luma = average[0] * 0.2126 + average[1] * 0.7152 + average[2] * 0.0722;
    let offsets = average.map(|channel| channel - luma);
    let largest_offset = offsets
        .iter()
        .map(|offset| offset.abs())
        .fold(0.0_f32, f32::max);
    let chroma_scale = if largest_offset > 0.0 {
        (14.0 / largest_offset).min(0.18)
    } else {
        0.0
    };
    let muted = offsets.map(|offset| (32.0 + offset * chroma_scale).clamp(18.0, 50.0) as u8);
    Color32::from_rgb(muted[0], muted[1], muted[2])
}

pub(crate) fn text_on_backdrop(color: Color32) -> Color32 {
    let luminance = f32::from(color.r()) * 0.2126
        + f32::from(color.g()) * 0.7152
        + f32::from(color.b()) * 0.0722;
    if luminance >= 145.0 {
        Color32::from_rgb(24, 25, 28)
    } else {
        Color32::from_rgb(242, 243, 246)
    }
}

#[derive(Clone, Copy)]
struct ThemePalette {
    accent: Color32,
    accent_bright: Color32,
    hyperlink: Color32,
    border: Color32,
    panel: Color32,
    window: Color32,
    faint: Color32,
    extreme: Color32,
    inactive: Color32,
    inactive_stroke: Color32,
    hovered: Color32,
    hovered_stroke: Color32,
    open: Color32,
    open_stroke: Color32,
}

const fn platform_control_height(android: bool) -> f32 {
    if android {
        ANDROID_CONTROL_HEIGHT
    } else {
        DESKTOP_CONTROL_HEIGHT
    }
}

#[cfg(any(target_os = "android", test))]
const fn platform_floating_action_edge(android: bool) -> f32 {
    if android {
        52.0
    } else {
        46.0
    }
}

pub(crate) fn toolbar_icon_size() -> Vec2 {
    Vec2::splat(TOOLBAR_ICON_EDGE)
}

#[cfg(not(target_os = "android"))]
pub(crate) fn tool_rail_icon_size() -> Vec2 {
    Vec2::splat(TOOL_RAIL_ICON_EDGE)
}

pub(crate) fn prepare_toolbar(ui: &mut Ui) {
    ui.set_min_height(TOOLBAR_HEIGHT);
    ui.spacing_mut().interact_size.y = CONTROL_HEIGHT;
    ui.spacing_mut().item_spacing = egui::vec2(SPACE_SM, SPACE_XS);
}

pub(crate) fn toolbar_row<R>(
    ui: &mut Ui,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    let size = egui::vec2(ui.available_width().max(1.0), TOOLBAR_HEIGHT);
    ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| {
        prepare_toolbar(ui);
        add_contents(ui)
    })
}

pub(crate) fn toolbar_title(ui: &mut Ui, title: impl Into<RichText>) -> Response {
    ui.label(title.into().strong().size(PANEL_TITLE_TEXT_SIZE))
}

#[cfg(test)]
pub(crate) fn panel_title(ui: &mut Ui, title: impl Into<RichText>) -> InnerResponse<Response> {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let title = ui
            .allocate_ui_with_layout(
                egui::vec2(ui.available_width().max(1.0), PANEL_TITLE_HEIGHT),
                Layout::left_to_right(Align::Center),
                |ui| ui.label(title.into().strong().size(PANEL_TITLE_TEXT_SIZE)),
            )
            .inner;
        ui.separator();
        title
    })
}

pub(crate) fn toolbar_frame(ui: &Ui) -> Frame {
    let compact = is_compact_portrait(ui);
    let stroke = if cfg!(target_os = "android") {
        Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color)
    } else {
        Stroke::NONE
    };
    Frame::new()
        .fill(ui.visuals().panel_fill)
        .inner_margin(Margin::symmetric(
            if compact { 10 } else { CONTENT_MARGIN },
            if compact { 4 } else { 6 },
        ))
        .stroke(stroke)
        .corner_radius(0.0)
}

pub(crate) fn panel_frame(ui: &Ui) -> Frame {
    Frame::new()
        .fill(ui.visuals().panel_fill)
        .inner_margin(Margin::same(content_margin(ui)))
        .stroke(Stroke::NONE)
}

pub(crate) fn workspace_frame(ui: &Ui) -> Frame {
    Frame::new()
        .fill(ui.visuals().window_fill)
        .inner_margin(Margin::same(content_margin(ui)))
        .stroke(Stroke::NONE)
}

pub(crate) fn card_frame(ui: &Ui) -> Frame {
    Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .inner_margin(Margin::same(content_margin(ui) + 2))
        .corner_radius(CARD_RADIUS)
        .stroke(Stroke::new(
            1.0,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
}

pub(crate) fn content_card<R>(
    ui: &mut Ui,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    let frame_width = f32::from(content_margin(ui)) * 2.0 + 6.0;
    let inner_width = (ui.available_width() - frame_width).max(1.0);
    card_frame(ui).show(ui, |ui| {
        ui.set_width(inner_width);
        ui.set_max_width(inner_width);
        add_contents(ui)
    })
}

pub(crate) fn card_header<R>(
    ui: &mut Ui,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    let horizontal_margin = content_margin(ui) + 2;
    let frame_width = f32::from(content_margin(ui)) * 2.0 + 6.0;
    let inner_width = (ui.available_width() - frame_width).max(1.0);
    Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .inner_margin(Margin::symmetric(horizontal_margin, 10))
        .corner_radius(CARD_RADIUS)
        .stroke(Stroke::new(
            1.0,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
        .show(ui, |ui| {
            ui.set_width(inner_width);
            ui.set_max_width(inner_width);
            add_contents(ui)
        })
}

pub(crate) fn section_card<R>(
    ui: &mut Ui,
    title: impl Into<RichText>,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    content_card(ui, |ui| {
        ui.strong(title);
        add_contents(ui)
    })
}

pub(crate) fn section_card_with_help<R>(
    ui: &mut Ui,
    title: impl Into<RichText>,
    help: &str,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    content_card(ui, |ui| {
        strong_with_help(ui, title, help);
        add_contents(ui)
    })
}

pub(crate) fn section_separator(ui: &mut Ui) -> Response {
    let extra_space = (SPACE_SM - ui.spacing().item_spacing.y).max(0.0);
    ui.add_space(extra_space);
    let response = ui.separator();
    ui.add_space(extra_space);
    response
}

pub(crate) fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);

    apply(ctx, UiDesign::default());
}

pub(crate) fn apply(ctx: &egui::Context, design: UiDesign) {
    let palette = design.palette();
    let theme = if design.is_dark() {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    };

    let mut visuals = if design.is_dark() {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };

    visuals.panel_fill = palette.panel;
    visuals.window_fill = palette.window;
    visuals.faint_bg_color = palette.faint;
    visuals.extreme_bg_color = palette.extreme;
    visuals.code_bg_color = palette.faint;
    visuals.selection.bg_fill = palette.accent;
    let active_text = if design.is_dark() {
        Color32::WHITE
    } else {
        Color32::from_rgb(30, 32, 37)
    };
    visuals.selection.stroke = Stroke::new(1.0, active_text);
    visuals.hyperlink_color = palette.hyperlink;
    visuals.window_stroke = Stroke::new(1.0, palette.border);
    visuals.window_corner_radius = 10.0.into();
    visuals.window_shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(if design.is_dark() { 72 } else { 30 }),
    };
    visuals.popup_shadow = egui::epaint::Shadow {
        offset: [0, 4],
        blur: 14,
        spread: 0,
        color: Color32::from_black_alpha(if design.is_dark() { 82 } else { 34 }),
    };
    visuals.menu_corner_radius = CARD_RADIUS.into();

    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.border);
    visuals.widgets.noninteractive.corner_radius = CARD_RADIUS.into();
    visuals.widgets.inactive.bg_fill = palette.inactive;
    visuals.widgets.inactive.weak_bg_fill = palette.inactive;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, palette.inactive_stroke);
    visuals.widgets.inactive.corner_radius = CARD_RADIUS.into();
    visuals.widgets.hovered.bg_fill = palette.hovered;
    visuals.widgets.hovered.weak_bg_fill = palette.hovered;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, palette.hovered_stroke);
    visuals.widgets.hovered.corner_radius = CARD_RADIUS.into();
    visuals.widgets.active.bg_fill = palette.accent;
    visuals.widgets.active.weak_bg_fill = palette.accent;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, palette.accent_bright);
    visuals.widgets.active.fg_stroke.color = active_text;
    visuals.widgets.active.corner_radius = CARD_RADIUS.into();
    visuals.widgets.open.bg_fill = palette.open;
    visuals.widgets.open.weak_bg_fill = palette.open;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, palette.open_stroke);
    visuals.widgets.open.corner_radius = CARD_RADIUS.into();
    visuals.interact_cursor = Some(egui::CursorIcon::PointingHand);

    let mut style = (*ctx.style_of(theme)).clone();
    style.visuals = visuals;
    #[cfg(all(target_os = "android", debug_assertions))]
    {
        // Debug APKs are also used for normal editing. Egui enables these
        // visual diagnostics by default in debug builds: tab changes flash
        // red ID-change outlines, and fractional layouts draw orange edges.
        style.debug.warn_if_rect_changes_id = false;
        style.debug.show_unaligned = false;
    }
    style
        .text_styles
        .insert(egui::TextStyle::Heading, egui::FontId::proportional(20.0));
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(13.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(13.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, egui::FontId::proportional(11.0));
    style.spacing.slider_width = 220.0;
    style.spacing.item_spacing = egui::vec2(SPACE_SM, SPACE_SM);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.interact_size.y = CONTROL_HEIGHT;
    style.spacing.window_margin = Margin::same(DIALOG_MARGIN);
    style.spacing.menu_margin = Margin::same(SPACE_SM as i8);
    style.spacing.indent = SPACE_LG;
    ctx.set_style_of(theme, style);
    ctx.set_theme(theme);
    ctx.request_repaint();
}

#[cfg(test)]
mod tests {
    use super::{
        platform_control_height, platform_floating_action_edge, UiDesign, ANDROID_CONTROL_HEIGHT,
        CONTROL_HEIGHT, DESKTOP_CONTROL_HEIGHT, FLOATING_ACTION_EDGE, FLOATING_ACTION_MARGIN,
    };

    #[test]
    fn obsidian_blue_is_the_default_and_dark_accents_are_distinct() {
        assert_eq!(UiDesign::default(), UiDesign::ObsidianBlue);
        assert_eq!(
            serde_json::from_str::<UiDesign>(r#""midnight_pink""#).unwrap(),
            UiDesign::ObsidianBlue
        );
        assert_eq!(
            serde_json::from_str::<UiDesign>(r#""graphite_mint""#).unwrap(),
            UiDesign::ObsidianRed
        );

        let blue = UiDesign::ObsidianBlue.palette().accent;
        let red = UiDesign::ObsidianRed.palette().accent;
        assert!(blue.b() > blue.r() && blue.b() > blue.g());
        assert!(red.r() > red.g() && red.r() > red.b());
        assert_ne!(blue, red);
    }

    #[test]
    fn android_widgets_request_the_full_touch_target_height() {
        assert_eq!(platform_control_height(true), ANDROID_CONTROL_HEIGHT);
        assert_eq!(platform_control_height(false), DESKTOP_CONTROL_HEIGHT);
    }

    #[test]
    fn compact_density_is_limited_to_android_portrait() {
        let portrait = eframe::egui::vec2(411.0, 891.0);
        let landscape = eframe::egui::vec2(891.0, 411.0);
        assert!(super::compact_portrait_for_platform(portrait, true));
        assert!(!super::compact_portrait_for_platform(landscape, true));
        assert!(!super::compact_portrait_for_platform(portrait, false));
    }

    #[test]
    fn floating_actions_use_platform_size_and_standard_inset() {
        assert!(platform_floating_action_edge(true) > ANDROID_CONTROL_HEIGHT);
        assert!(platform_floating_action_edge(false) > DESKTOP_CONTROL_HEIGHT);

        let bounds = eframe::egui::Rect::from_min_size(
            eframe::egui::pos2(10.0, 20.0),
            eframe::egui::vec2(300.0, 400.0),
        );
        let rect = super::floating_action_rect(bounds);
        assert_eq!(rect.size(), eframe::egui::Vec2::splat(FLOATING_ACTION_EDGE));
        assert_eq!(
            rect.right_bottom(),
            bounds.right_bottom() - eframe::egui::Vec2::splat(FLOATING_ACTION_MARGIN)
        );
    }

    #[test]
    fn disabled_interaction_state_overrides_other_flags() {
        assert_eq!(
            super::interaction_visual_state(false, true, true, true, true),
            super::InteractionVisualState::Disabled
        );
    }

    #[test]
    fn active_selected_and_focus_states_have_stable_priority() {
        assert_eq!(
            super::interaction_visual_state(true, true, true, true, true),
            super::InteractionVisualState::Active
        );
        assert_eq!(
            super::interaction_visual_state(true, true, false, true, true),
            super::InteractionVisualState::Selected
        );
        assert_eq!(
            super::interaction_visual_state(true, false, false, false, true),
            super::InteractionVisualState::Focused
        );
        assert_eq!(
            super::interaction_visual_state(true, false, false, true, false),
            super::InteractionVisualState::Hovered
        );
        assert_eq!(
            super::interaction_visual_state(true, false, false, false, false),
            super::InteractionVisualState::Inactive
        );
    }

    #[test]
    fn segmented_buttons_honor_their_assigned_width() {
        eframe::egui::__run_test_ui(|ui| {
            let width = 42.0;
            let response = super::segmented_button(ui, "Long segment label", false, width);
            assert_eq!(response.rect.width(), width);
            assert_eq!(response.rect.height(), CONTROL_HEIGHT);
        });
    }

    #[test]
    fn property_rows_share_control_height_and_vertical_alignment() {
        eframe::egui::__run_test_ui(|ui| {
            ui.set_width(360.0);
            let row = super::property_row(ui, "Mode", |ui| {
                ui.add_sized([96.0, CONTROL_HEIGHT], eframe::egui::Button::new("Value"))
            });
            assert_eq!(row.response.rect.height(), CONTROL_HEIGHT);
            assert!((row.inner.rect.center().y - row.response.rect.center().y).abs() < 0.001);
        });
    }

    #[test]
    fn form_rows_expand_in_compact_mode_and_cap_controls_after_breakpoint() {
        fn offered_control_width(width: f32) -> f32 {
            let ctx = eframe::egui::Context::default();
            let mut offered = 0.0;
            let _ = ctx.run_ui(
                eframe::egui::RawInput {
                    screen_rect: Some(eframe::egui::Rect::from_min_size(
                        eframe::egui::Pos2::ZERO,
                        eframe::egui::vec2(width, 200.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    ui.set_width(width);
                    super::form_row(ui, "Quality", 220.0, |ui, control_width| {
                        offered = control_width;
                        ui.add_sized(
                            [control_width, CONTROL_HEIGHT],
                            eframe::egui::Button::new("Value"),
                        );
                    });
                },
            );
            offered
        }

        assert!(offered_control_width(400.0) > 300.0);
        assert!((offered_control_width(600.0) - 220.0).abs() < 0.001);
        assert!((offered_control_width(900.0) - 220.0).abs() < 0.001);
    }

    #[test]
    fn full_width_buttons_use_standard_control_height() {
        eframe::egui::__run_test_ui(|ui| {
            ui.set_width(280.0);
            let response = super::full_width_button(ui, "Continue");
            assert_eq!(response.rect.height(), CONTROL_HEIGHT);
            assert!((response.rect.width() - 280.0).abs() < 0.001);
        });
    }

    fn key_press(key: eframe::egui::Key) -> eframe::egui::Event {
        eframe::egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: eframe::egui::Modifiers::NONE,
        }
    }

    fn run_dialog_key(
        key: eframe::egui::Key,
        keyboard: super::DialogKeyboard,
        confirm_enabled: bool,
        consume_before_fallback: bool,
    ) -> (super::DialogAction, bool, bool) {
        let ctx = eframe::egui::Context::default();
        let mut action = super::DialogAction::None;
        let mut consumed_before_fallback = false;
        let mut key_remains = false;
        let _ = ctx.run_ui(
            eframe::egui::RawInput {
                events: vec![key_press(key)],
                ..Default::default()
            },
            |ui| {
                if consume_before_fallback {
                    consumed_before_fallback =
                        ui.input_mut(|input| input.consume_key(eframe::egui::Modifiers::NONE, key));
                }
                action = super::dialog_keyboard_action(ui, keyboard, confirm_enabled);
                key_remains =
                    ui.input_mut(|input| input.consume_key(eframe::egui::Modifiers::NONE, key));
            },
        );
        (action, consumed_before_fallback, key_remains)
    }

    fn run_confirmation_key(
        key: eframe::egui::Key,
        keyboard: super::DialogKeyboard,
        destructive: bool,
        consume_before_buttons: bool,
    ) -> (super::DialogAction, bool, bool) {
        let ctx = eframe::egui::Context::default();
        let mut action = super::DialogAction::None;
        let mut consumed_before_buttons = false;
        let mut key_remains = false;
        let _ = ctx.run_ui(
            eframe::egui::RawInput {
                events: vec![key_press(key)],
                ..Default::default()
            },
            |ui| {
                if consume_before_buttons {
                    consumed_before_buttons =
                        ui.input_mut(|input| input.consume_key(eframe::egui::Modifiers::NONE, key));
                }
                action = super::dialog_confirmation_buttons(
                    ui,
                    "Cancel",
                    "Confirm",
                    true,
                    destructive,
                    keyboard,
                );
                key_remains =
                    ui.input_mut(|input| input.consume_key(eframe::egui::Modifiers::NONE, key));
            },
        );
        (action, consumed_before_buttons, key_remains)
    }

    #[test]
    fn close_only_does_not_consume_enter() {
        let (action, _, key_remains) = run_dialog_key(
            eframe::egui::Key::Enter,
            super::DialogKeyboard::CLOSE_ONLY,
            true,
            false,
        );

        assert_eq!(action, super::DialogAction::None);
        assert!(key_remains);
    }

    #[test]
    fn enter_fallback_confirms_simple_forms() {
        let (action, _, key_remains) = run_confirmation_key(
            eframe::egui::Key::Enter,
            super::DialogKeyboard::CONFIRM_ON_ENTER,
            false,
            false,
        );

        assert_eq!(action, super::DialogAction::Confirm);
        assert!(!key_remains);
    }

    #[test]
    fn destructive_dialogs_disable_global_enter_confirmation() {
        let (action, _, key_remains) = run_confirmation_key(
            eframe::egui::Key::Enter,
            super::DialogKeyboard::CONFIRM_ON_ENTER,
            true,
            false,
        );

        assert_eq!(action, super::DialogAction::None);
        assert!(key_remains);
    }

    #[test]
    fn consumed_enter_does_not_trigger_confirmation_fallback() {
        let (action, consumed_before_buttons, key_remains) = run_confirmation_key(
            eframe::egui::Key::Enter,
            super::DialogKeyboard::CONFIRM_ON_ENTER,
            false,
            true,
        );

        assert!(consumed_before_buttons);
        assert_eq!(action, super::DialogAction::None);
        assert!(!key_remains);
    }

    #[test]
    fn handled_keys_do_not_trigger_a_second_dialog_action() {
        for key in [eframe::egui::Key::Enter, eframe::egui::Key::Escape] {
            let (action, consumed_before_fallback, key_remains) =
                run_dialog_key(key, super::DialogKeyboard::CONFIRM_ON_ENTER, true, true);

            assert!(consumed_before_fallback);
            assert_eq!(action, super::DialogAction::None);
            assert!(!key_remains);
        }
    }

    #[test]
    fn destructive_menu_items_keep_default_button_geometry() {
        eframe::egui::__run_test_ui(|ui| {
            let (normal, destructive) = ui
                .horizontal(|ui| {
                    let normal = ui.button("Delete");
                    let destructive = super::destructive_menu_item(ui, "Delete");
                    (normal.rect.size(), destructive.rect.size())
                })
                .inner;

            assert_eq!(normal, destructive);
        });
    }

    #[test]
    fn dialog_initial_focus_is_requested_only_once() {
        let mut focus_requested = false;
        assert!(super::take_initial_focus_request(&mut focus_requested));
        assert!(focus_requested);
        assert!(!super::take_initial_focus_request(&mut focus_requested));
    }

    #[test]
    fn dialog_buttons_share_height_and_cancel_precedes_confirm() {
        eframe::egui::__run_test_ui(|ui| {
            let (cancel, confirm, destructive) = super::dialog_button_row(ui, |ui| {
                let cancel = super::secondary_button(ui, "Cancel").rect;
                let confirm = super::primary_action_button(ui, "Save").rect;
                let destructive = super::destructive_button(ui, "Delete").rect;
                (cancel, confirm, destructive)
            })
            .inner;

            assert_eq!(cancel.height(), CONTROL_HEIGHT);
            assert_eq!(confirm.height(), CONTROL_HEIGHT);
            assert_eq!(destructive.height(), CONTROL_HEIGHT);
            assert!(cancel.left() < confirm.left());
            assert!(confirm.left() < destructive.left());
        });
    }

    #[test]
    fn dialog_margin_uses_the_theme_constant() {
        let ctx = eframe::egui::Context::default();
        super::apply(&ctx, UiDesign::default());
        assert_eq!(
            ctx.style_of(ctx.theme()).spacing.window_margin,
            eframe::egui::Margin::same(super::DIALOG_MARGIN)
        );
    }

    #[test]
    fn panel_title_text_is_vertically_centered() {
        eframe::egui::__run_test_ui(|ui| {
            ui.set_width(320.0);
            let row_top = ui.cursor().top();
            let title = super::panel_title(ui, "Edit").inner;
            let expected_center = row_top + super::PANEL_TITLE_HEIGHT * 0.5;
            assert!((title.rect.center().y - expected_center).abs() < 0.001);
        });
    }

    #[test]
    fn toolbar_row_centers_labels_and_actions_on_the_same_axis() {
        eframe::egui::__run_test_ui(|ui| {
            ui.set_width(320.0);
            let (label, action) = super::toolbar_row(ui, |ui| {
                let label = ui.strong("Section");
                let action = ui
                    .with_layout(
                        eframe::egui::Layout::right_to_left(eframe::egui::Align::Center),
                        |ui| {
                            ui.add_sized(super::toolbar_icon_size(), eframe::egui::Button::new("R"))
                        },
                    )
                    .inner;
                (label.rect, action.rect)
            })
            .inner;

            assert!((label.center().y - action.center().y).abs() < 0.001);
            assert_eq!(action.size(), super::toolbar_icon_size());
        });
    }

    #[test]
    fn panel_and_workspace_content_share_the_same_inset() {
        eframe::egui::__run_test_ui(|ui| {
            assert_eq!(
                super::panel_frame(ui).inner_margin,
                super::workspace_frame(ui).inner_margin
            );
        });
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn desktop_toolbar_removes_its_border_without_removing_padding() {
        eframe::egui::__run_test_ui(|ui| {
            let frame = super::toolbar_frame(ui);
            assert_eq!(frame.inner_margin.left, super::CONTENT_MARGIN);
            assert_eq!(frame.inner_margin.right, super::CONTENT_MARGIN);
            assert_eq!(frame.stroke, eframe::egui::Stroke::NONE);
        });
    }

    #[test]
    fn photo_matched_backdrops_are_distinct_but_quiet() {
        let red = super::adaptive_backdrop_from_rgba(&[240, 30, 20, 255].repeat(32));
        let blue = super::adaptive_backdrop_from_rgba(&[20, 50, 240, 255].repeat(32));

        assert_ne!(red, blue);
        for color in [red, blue] {
            assert!(color.r() >= 18 && color.r() <= 50);
            assert!(color.g() >= 18 && color.g() <= 50);
            assert!(color.b() >= 18 && color.b() <= 50);
        }
    }
}
