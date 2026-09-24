use crate::app::{
    AdjustmentSection, AppAction, CalibRawApp, ColorGradeTab, HslMixerColor, InpaintTool,
    MaskSection, SidebarTab, ToneCurveTab,
};
use crate::pipeline::{
    AdjustmentGroup, BrushMode, DenoiseQuality, ExportBitDepth, ExportFormat, ExportResizeMode,
    ExposureParams, LoadedRaw, LocalMask, MaskCombineMode, MaskComponent, MaskEffect,
    MaskEffectCategory, MaskGeometry, MaskKind, RetouchAlignment, MAX_LOCAL_MASKS,
    MAX_MASK_COMPONENTS, MAX_WHITE_BALANCE_TINT, MIN_WHITE_BALANCE_TINT,
};
use crate::ui::components::adjustment_slider::{
    adjustment_slider, adjustment_slider_with_reset, float_param_slider,
    gradient_adjustment_slider, gradient_adjustment_slider_with_reset, gradient_float_param_slider,
    hue_adjustment_slider, slider_scroll_locked, SliderGradient,
};
use crate::ui::components::color_grading::color_grading_editor;
use crate::ui::components::hsl_mixer::hsl_mixer;
use crate::ui::components::tone_curve_editor::{tone_curve_channel_editor, ToneCurveChannels};
use crate::ui::layout::ScreenLayout;
use eframe::egui::{self, Ui};

pub(crate) struct Sidebar;

mod adjustment_cards;
mod histogram;
mod mask_effects;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MaskCardSize {
    Group,
    Submask,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MaskStripOrientation {
    Horizontal,
    Vertical,
}

impl MaskCardSize {
    fn card_size(self) -> egui::Vec2 {
        match self {
            Self::Group => egui::vec2(68.0, 72.0),
            Self::Submask => egui::vec2(56.0, 62.0),
        }
    }

    fn image_edge(self) -> f32 {
        match self {
            Self::Group => 54.0,
            Self::Submask => 44.0,
        }
    }

    fn label_font_size(self) -> f32 {
        match self {
            Self::Group => 9.5,
            Self::Submask => 8.5,
        }
    }

    fn create_button_size(self, orientation: MaskStripOrientation) -> egui::Vec2 {
        const THIN_EDGE: f32 = crate::ui::theme::CONTROL_HEIGHT;
        let card = self.card_size();
        match orientation {
            MaskStripOrientation::Horizontal => egui::vec2(THIN_EDGE, card.y),
            MaskStripOrientation::Vertical => egui::vec2(card.x, THIN_EDGE),
        }
    }
}

impl Sidebar {
    #[cfg(not(target_os = "android"))]
    pub(crate) const DESKTOP_TOOL_RAIL_WIDTH: f32 = 60.0;
    #[cfg(target_os = "android")]
    pub(crate) const ANDROID_LANDSCAPE_TOOL_RAIL_WIDTH: f32 = 72.0;
    const MASK_THUMBNAIL_EDGE: u32 = 64;
    pub(crate) const VERTICAL_MASK_STRIP_HEIGHT: f32 = 92.0;
    pub(crate) const HORIZONTAL_MASK_STRIP_WIDTH: f32 = 92.0;
    const CONTEXT_TAB_WIDTH: f32 = 64.0;
}

include!("sidebar/navigation.rs");
mod masks;
include!("sidebar/inpainting.rs");
include!("sidebar/export.rs");
include!("sidebar/info.rs");
include!("sidebar/develop.rs");
include!("sidebar/crop.rs");

#[cfg(test)]
mod tests {
    use super::masks::{mask_component_badge, mask_creation_icon};
    use super::{
        mobile_tab_icon_geometry, mobile_tab_text_geometry, MaskCardSize, MaskCombineMode,
        MaskStripOrientation,
    };
    use eframe::egui;

    #[test]
    fn mobile_effect_tabs_keep_add_after_every_component() {
        use crate::pipeline::{EffectComponent, MaskEffect};

        fn label_rect(shapes: &[egui::epaint::ClippedShape], label: &str) -> egui::Rect {
            fn find(shape: &egui::Shape, label: &str) -> Option<egui::Rect> {
                match shape {
                    egui::Shape::Text(text) if text.galley.text() == label => {
                        Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
                    }
                    egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, label)),
                    _ => None,
                }
            }
            shapes
                .iter()
                .find_map(|shape| find(&shape.shape, label))
                .expect("mobile effect tab label")
        }

        let ctx = egui::Context::default();
        crate::ui::theme::install(&ctx);
        let mut components = vec![
            EffectComponent::new(MaskEffect::Blur),
            EffectComponent::new(MaskEffect::Glow),
        ];
        let mut selection = None;
        let render = |components: &mut Vec<EffectComponent>,
                      selection: &mut Option<MaskEffect>,
                      events: Vec<egui::Event>| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 100.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.horizontal(|ui| {
                        assert!(!super::Sidebar::show_mobile_effect_tabs(
                            ui, components, selection, 44.0, true,
                        ));
                    });
                },
            )
            .shapes
        };

        let shapes = render(&mut components, &mut selection, Vec::new());
        assert!(label_rect(&shapes, "Blur").left() < label_rect(&shapes, "Glow").left());
        assert!(label_rect(&shapes, "Glow").left() < label_rect(&shapes, "Add").left());

        let glow = label_rect(&shapes, "Glow").center();
        let click = |pressed| {
            vec![
                egui::Event::PointerMoved(glow),
                egui::Event::PointerButton {
                    pos: glow,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        render(&mut components, &mut selection, click(true));
        render(&mut components, &mut selection, click(false));
        assert_eq!(selection, Some(MaskEffect::Glow));

        components.push(EffectComponent::new(MaskEffect::Fog));
        let shapes = render(&mut components, &mut selection, Vec::new());
        assert!(label_rect(&shapes, "Glow").left() < label_rect(&shapes, "Fog").left());
        assert!(label_rect(&shapes, "Fog").left() < label_rect(&shapes, "Add").left());
    }

    #[test]
    fn export_action_stays_visible_above_scrolling_settings() {
        for size in [
            egui::vec2(320.0, 180.0),
            egui::vec2(360.0, 600.0),
            egui::vec2(420.0, 800.0),
        ] {
            let ctx = egui::Context::default();
            let mut previous_button = None;
            for offset in [0.0, 300.0, 1000.0] {
                let _ = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ui| {
                        let viewport = ui.available_rect_before_wrap();
                        let button = super::show_export_action_panel(ui, |ui| {
                            ui.add_sized(
                                [ui.available_width(), crate::ui::theme::CONTROL_HEIGHT],
                                egui::Button::new("Export…"),
                            )
                        })
                        .inner
                        .rect;
                        let scroll = egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .vertical_scroll_offset(offset)
                            .show(ui, |ui| {
                                ui.allocate_space(egui::vec2(100.0, 2000.0));
                            });
                        assert!(viewport.contains_rect(button));
                        assert!(scroll.inner_rect.bottom() <= button.top());
                        assert!((viewport.bottom() - button.bottom() - 8.0).abs() < 1.0);
                        if let Some(previous) = previous_button {
                            assert_eq!(button, previous);
                        }
                        previous_button = Some(button);
                    },
                );
            }
        }
    }

    #[test]
    fn export_format_changes_keep_bit_depth_supported() {
        use crate::pipeline::{ExportBitDepth, ExportFormat, ExportSettings};
        let mut settings = ExportSettings {
            bit_depth: ExportBitDepth::Float32Linear,
            ..ExportSettings::default()
        };
        super::enforce_export_bit_depth(ExportFormat::Tiff, &mut settings);
        assert_eq!(settings.bit_depth, ExportBitDepth::Float32Linear);
        super::enforce_export_bit_depth(ExportFormat::Png, &mut settings);
        assert_eq!(settings.bit_depth, ExportBitDepth::Sixteen);
        super::enforce_export_bit_depth(ExportFormat::Jpeg, &mut settings);
        assert_eq!(settings.bit_depth, ExportBitDepth::Eight);
        settings.bit_depth = ExportBitDepth::Float32Linear;
        super::enforce_export_bit_depth(ExportFormat::JpegXl, &mut settings);
        assert_eq!(settings.bit_depth, ExportBitDepth::Sixteen);
    }

    #[test]
    fn mask_badges_match_base_and_combine_semantics() {
        assert_eq!(mask_component_badge(0, MaskCombineMode::Subtract), "BASE");
        assert_eq!(
            mask_component_badge(1, MaskCombineMode::Add),
            egui_phosphor::regular::PLUS
        );
        assert_eq!(
            mask_component_badge(1, MaskCombineMode::Subtract),
            egui_phosphor::regular::MINUS
        );
        assert_eq!(
            mask_component_badge(1, MaskCombineMode::Intersect),
            egui_phosphor::regular::INTERSECT
        );
    }

    #[test]
    fn mask_creation_controls_use_a_compact_plus_icon() {
        assert_eq!(mask_creation_icon(), egui_phosphor::regular::PLUS);
    }

    #[test]
    fn mask_creation_controls_are_thin_along_the_strip_axis() {
        assert_eq!(
            MaskCardSize::Group.create_button_size(MaskStripOrientation::Horizontal),
            eframe::egui::vec2(crate::ui::theme::CONTROL_HEIGHT, 72.0)
        );
        assert_eq!(
            MaskCardSize::Submask.create_button_size(MaskStripOrientation::Horizontal),
            eframe::egui::vec2(crate::ui::theme::CONTROL_HEIGHT, 62.0)
        );
        assert_eq!(
            MaskCardSize::Group.create_button_size(MaskStripOrientation::Vertical),
            eframe::egui::vec2(68.0, crate::ui::theme::CONTROL_HEIGHT)
        );
        assert_eq!(
            MaskCardSize::Submask.create_button_size(MaskStripOrientation::Vertical),
            eframe::egui::vec2(56.0, crate::ui::theme::CONTROL_HEIGHT)
        );
    }

    #[test]
    fn mobile_tab_icon_and_label_stack_is_vertically_centered() {
        for height in [44.0, 48.0, 52.0, 56.0] {
            let (icon_size, label_size, icon_center, label_center) =
                mobile_tab_text_geometry(height);
            let stack_top = icon_center - icon_size * 0.5;
            let stack_bottom = label_center + label_size * 0.5;
            assert!(((stack_top + stack_bottom) * 0.5 - height * 0.5).abs() < 0.001);
            assert!(icon_center < label_center);
        }
    }

    #[test]
    fn unlabeled_mobile_tab_icon_is_centered() {
        for height in [44.0, 48.0, 52.0, 56.0] {
            let (icon_size, icon_center) = mobile_tab_icon_geometry(height, false);
            assert!((icon_center - height * 0.5).abs() < 0.001);
            assert!((21.0..=25.0).contains(&icon_size));
        }
    }
}
