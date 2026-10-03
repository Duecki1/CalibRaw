use eframe::egui::{self, Color32};
use serde::{Deserialize, Serialize};

#[cfg(target_os = "android")]
pub(crate) use moduwu_design::floating_action_button;
#[cfg(target_os = "android")]
pub(crate) use moduwu_design::floating_action_rect;
pub(crate) use moduwu_design::{
    action_row, card_gap, card_header, checkbox_with_help, combo_box, content_card, context_menu,
    context_menu_item, destructive_menu_item, dropdown_menu, dropdown_submenu, form_combo,
    form_combo_with_help, form_row, full_width_button, heading_with_help, interaction_visuals,
    interaction_visuals_for_flags, is_compact_portrait, menu_item, navigation_row, panel_frame,
    prepare_toolbar, primary_action_button, primary_button, progress_card_header, property_row,
    responsive_combo_box, secondary_button, secondary_button_enabled, section_card,
    section_card_with_help, section_separator, segmented_button, singleline_text_edit,
    strong_with_help, toggle_button, toolbar_button, toolbar_frame, toolbar_icon_size, toolbar_row,
    toolbar_title, workspace_frame, InteractionVisualState, CARD_RADIUS, CONTROL_HEIGHT,
    PANEL_TITLE_TEXT_SIZE, SPACE_MD, SPACE_SM, SPACE_XS, SPACE_XXS, TOOLBAR_HEIGHT,
    TOOLBAR_ICON_EDGE,
};
pub(crate) use moduwu_design::{
    dialog_button_row, dialog_confirmation_buttons, dialog_keyboard_action, dialog_window,
    request_initial_focus, DialogAction, DialogKeyboard, DIALOG_WIDTH_DEFAULT, DIALOG_WIDTH_FORM,
    DIALOG_WIDTH_LARGE, DIALOG_WIDTH_NARROW, DIALOG_WIDTH_WIDE,
};
#[cfg(not(target_os = "android"))]
pub(crate) use moduwu_design::{tab_button, tool_rail_icon_size, CONTENT_MARGIN};
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
pub(crate) const COLORFULNESS_BLUE: Color32 = Color32::from_rgb(73, 130, 232);
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

    const fn design(self) -> moduwu_design::Design {
        use moduwu_design::Design;
        match self {
            Self::ObsidianBlue => Design::ObsidianBlue,
            Self::ObsidianRed => Design::ObsidianRed,
            Self::Porcelain => Design::Porcelain,
            Self::DaylightBlue => Design::DaylightBlue,
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        self.design().label()
    }

    pub(crate) const fn description(self) -> &'static str {
        self.design().description()
    }

    #[cfg(target_os = "android")]
    pub(crate) const fn is_dark(self) -> bool {
        self.design().is_dark()
    }

    #[cfg(test)]
    const fn palette(self) -> moduwu_design::Palette {
        self.design().palette()
    }

    const fn theme(self) -> moduwu_design::Theme {
        self.design().theme()
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

/// The single-line text field used in every dialog. It fills the dialog's
/// width: egui caps a field's desired width, margins included, at the space
/// available, so the field can never widen the window. Sizing a field from
/// `ui.available_width()` instead grows the window a little every frame.
pub(crate) fn dialog_text_edit<'t>(
    text: &'t mut dyn egui::TextBuffer,
    id_salt: impl egui::AsIdSalt,
) -> egui::TextEdit<'t> {
    singleline_text_edit(text)
        .desired_width(f32::INFINITY)
        .id_salt(id_salt)
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

pub(crate) fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
    apply(ctx, UiDesign::default());
}

pub(crate) fn apply(ctx: &egui::Context, design: UiDesign) {
    design.theme().apply(ctx);
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn app_theme_selection_delegates_to_shared_theme_application() {
        let ctx = egui::Context::default();
        apply(&ctx, UiDesign::ObsidianRed);
        let style = ctx.style_of(ctx.theme());

        assert!(style.visuals.dark_mode);
        assert_eq!(
            style.visuals.widgets.active.bg_fill,
            UiDesign::ObsidianRed.palette().accent
        );
    }

    #[test]
    fn photo_matched_backdrops_are_distinct_but_quiet() {
        let red = adaptive_backdrop_from_rgba(&[240, 30, 20, 255].repeat(32));
        let blue = adaptive_backdrop_from_rgba(&[20, 50, 240, 255].repeat(32));

        assert_ne!(red, blue);
        for color in [red, blue] {
            assert!(color.r() >= 18 && color.r() <= 50);
            assert!(color.g() >= 18 && color.g() <= 50);
            assert!(color.b() >= 18 && color.b() <= 50);
        }
    }
}
