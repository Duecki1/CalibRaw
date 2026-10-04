use crate::appearance::UiDesign;
use eframe::egui::{self, Color32};

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

pub(crate) fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
    apply(ctx, UiDesign::default());
}

pub(crate) fn apply(ctx: &egui::Context, design: UiDesign) {
    design.design().theme().apply(ctx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_theme_selection_delegates_to_shared_theme_application() {
        let ctx = egui::Context::default();
        apply(&ctx, UiDesign::ObsidianRed);
        let style = ctx.style_of(ctx.theme());

        assert!(style.visuals.dark_mode);
        assert_eq!(
            style.visuals.widgets.active.bg_fill,
            UiDesign::ObsidianRed.design().palette().accent
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
