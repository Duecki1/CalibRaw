//! Feature tips: a short clip and one sentence, shown the first time a feature
//! is used. The app decides when a tip is offered (`app::tips`); this module
//! holds what each tip shows and draws the dialog.

mod animation;

pub(crate) use animation::TipAnimation;
use eframe::egui;

/// The largest share of the window height a clip may take, so the text and
/// buttons stay visible on short screens.
const MAX_ANIMATION_HEIGHT_FRACTION: f32 = 0.45;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tip {
    MasksTab,
    ObjectMask,
    DepthRange,
    PathMask,
    ColorRangeMask,
    LuminanceRangeMask,
}

impl Tip {
    #[cfg(test)]
    const ALL: [Self; 6] = [
        Self::MasksTab,
        Self::ObjectMask,
        Self::DepthRange,
        Self::PathMask,
        Self::ColorRangeMask,
        Self::LuminanceRangeMask,
    ];

    /// Saved in settings once the tip was seen; never change an existing key.
    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::MasksTab => "masks_tab",
            Self::ObjectMask => "object_mask",
            Self::DepthRange => "depth_range",
            Self::PathMask => "path_mask",
            Self::ColorRangeMask => "color_range_mask",
            Self::LuminanceRangeMask => "luminance_range_mask",
        }
    }

    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::MasksTab => "Masks",
            Self::ObjectMask => "Object mask",
            Self::DepthRange => "Depth range",
            Self::PathMask => "Freeform mask",
            Self::ColorRangeMask => "Color range mask",
            Self::LuminanceRangeMask => "Luminance range mask",
        }
    }

    const fn body(self) -> &'static str {
        match self {
            Self::MasksTab => {
                "Create a mask group with the plus card, then adjust only the masked area. \
                 Add sub-masks to extend, subtract from or intersect a group."
            }
            Self::ObjectMask => {
                "Paint roughly over the object you want to select. \
                 CalibRaw finds its outline; paint again to include more of it."
            }
            Self::DepthRange => {
                "Drag the solid handles to set Near and Far, the ends of the depth range. \
                 Drag the hollow handles to feather each end."
            }
            Self::PathMask => {
                "Click to add points around the subject; the outline closes itself. \
                 Drag while adding a point to curve its edge, then drag its handles to reshape it."
            }
            Self::ColorRangeMask => {
                "Click or drag on the photo to sample a color. \
                 Then use Tolerance to widen or narrow the selection, and Grow to expand or shrink it."
            }
            Self::LuminanceRangeMask => {
                "Drag the solid handles to set the luminance range, the darkest and brightest tones selected. \
                 Drag the hollow handles to feather each end."
            }
        }
    }

    /// An animated or still WebP, at most 640 px wide.
    pub(crate) const fn media(self) -> &'static [u8] {
        match self {
            Self::MasksTab => include_bytes!("../../assets/tips/masks-tab.webp"),
            Self::ObjectMask => include_bytes!("../../assets/tips/object-mask.webp"),
            Self::DepthRange => include_bytes!("../../assets/tips/depth-range.webp"),
            Self::PathMask => include_bytes!("../../assets/tips/path-mask.webp"),
            Self::ColorRangeMask => include_bytes!("../../assets/tips/color-range-mask.webp"),
            Self::LuminanceRangeMask => {
                include_bytes!("../../assets/tips/luminance-range-mask.webp")
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TipChoice {
    /// Close this tip; it is not shown again.
    Dismiss,
    /// Close this tip and show no further tips.
    DisableTips,
}

/// Shows `tip` with its clip, when it has one, and returns the user's choice.
pub(crate) fn show(
    ctx: &egui::Context,
    tip: Tip,
    animation: Option<&TipAnimation>,
) -> Option<TipChoice> {
    let mut choice = None;
    moduwu_design::Dialog::new(("calibraw-tip", tip.key()), tip.title())
        .size(moduwu_design::DialogSize::Wide)
        .show_with_footer(
            ctx,
            |ui| {
                if let Some(animation) = animation {
                    show_animation(ui, tip, animation);
                    ui.add_space(moduwu_design::SPACE_SM);
                }
                ui.label(tip.body());
            },
            |ui| {
                if moduwu_design::primary_action_button(ui, "Got it").clicked() {
                    choice = Some(TipChoice::Dismiss);
                }
                if moduwu_design::secondary_button(ui, "Don't show tips").clicked() {
                    choice = Some(TipChoice::DisableTips);
                }
                if choice.is_none()
                    && moduwu_design::dialog_keyboard_action(
                        ui,
                        moduwu_design::DialogKeyboard::CONFIRM_ON_ENTER,
                        true,
                    ) != moduwu_design::DialogAction::None
                {
                    choice = Some(TipChoice::Dismiss);
                }
            },
        );
    choice
}

/// Draws the clip centred at the body width, never enlarged.
fn show_animation(ui: &mut egui::Ui, tip: Tip, animation: &TipAnimation) {
    let size = animation.size();
    let max_height = ui.ctx().content_rect().height() * MAX_ANIMATION_HEIGHT_FRACTION;
    let scale = (ui.available_width() / size.x)
        .min(max_height / size.y)
        .min(1.0);
    ui.vertical_centered(|ui| {
        ui.add(
            egui::Image::new((animation.texture().id(), size * scale))
                .corner_radius(moduwu_design::CARD_RADIUS)
                .alt_text(tip.title()),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use image_webp::WebPDecoder;
    use std::collections::HashSet;
    use std::io::Cursor;

    const MAX_MEDIA_WIDTH: u32 = 640;
    const MAX_MEDIA_BYTES: usize = 1024 * 1024;
    const MAX_CLIP_MS: u64 = 12_000;

    #[test]
    fn keys_are_unique_and_stable() {
        let keys: HashSet<_> = Tip::ALL.iter().map(|tip| tip.key()).collect();
        assert_eq!(keys.len(), Tip::ALL.len());
        assert_eq!(Tip::MasksTab.key(), "masks_tab");
        assert_eq!(Tip::ObjectMask.key(), "object_mask");
        assert_eq!(Tip::DepthRange.key(), "depth_range");
        assert_eq!(Tip::PathMask.key(), "path_mask");
        assert_eq!(Tip::ColorRangeMask.key(), "color_range_mask");
        assert_eq!(Tip::LuminanceRangeMask.key(), "luminance_range_mask");
    }

    #[test]
    fn media_stays_within_budget() {
        for tip in Tip::ALL {
            let media = tip.media();
            assert!(media.len() <= MAX_MEDIA_BYTES, "{tip:?} is too large");
            let decoder = WebPDecoder::new(Cursor::new(media)).expect("tip media is WebP");
            assert!(
                decoder.dimensions().0 <= MAX_MEDIA_WIDTH,
                "{tip:?} is too wide"
            );
            assert!(
                decoder.loop_duration() <= MAX_CLIP_MS,
                "{tip:?} is too long"
            );
        }
    }
}
