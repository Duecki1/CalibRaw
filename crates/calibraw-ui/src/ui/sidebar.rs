use crate::app::{
    AdjustmentSection, AppAction, CalibRawApp, ColorGradeTab, DevelopCard, HslMixerColor,
    InpaintTool, MaskSection, SidebarTab, ToneCurveTab,
};
use crate::pipeline::{
    AdjustmentGroup, BrushMode, DenoiseQuality, ExportBitDepth, ExportFormat, ExportResizeMode,
    ExposureParams, LoadedRaw, LocalMask, MaskCombineMode, MaskComponent, MaskEffect, MaskGeometry,
    MaskKind, RetouchAlignment, WhiteBalance, MAX_LOCAL_MASKS, MAX_MASK_COMPONENTS,
};
use crate::ui::components::adjustment_slider::{
    float_param_slider, gradient_float_param_slider, hue_adjustment_slider, AdjustmentSlider,
    SliderGradient,
};
use crate::ui::components::color_grading::color_grading_editor;
use crate::ui::components::hsl_mixer::hsl_mixer;
use crate::ui::components::tone_curve_editor::{tone_curve_channel_editor, ToneCurveChannels};
use eframe::egui::{self, Ui};
use moduwu_design::ScreenLayout;

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

    /// Characters of the name a card shows.
    fn max_label_chars(self) -> usize {
        match self {
            Self::Group => 13,
            Self::Submask => 10,
        }
    }

    /// Font size, height and horizontal padding of the thumbnail badge.
    fn badge_metrics(self) -> (f32, f32, f32) {
        match self {
            Self::Group => (10.5, 18.0, 10.0),
            Self::Submask => (9.0, 16.0, 8.0),
        }
    }

    fn create_button_size(self, orientation: MaskStripOrientation) -> egui::Vec2 {
        const THIN_EDGE: f32 = moduwu_design::CONTROL_HEIGHT;
        let card = self.card_size();
        match orientation {
            MaskStripOrientation::Horizontal => egui::vec2(THIN_EDGE, card.y),
            MaskStripOrientation::Vertical => egui::vec2(card.x, THIN_EDGE),
        }
    }
}

impl Sidebar {
    pub(crate) const DOCKED_TOOL_RAIL_WIDTH: f32 = 60.0;
    pub(crate) const ANDROID_LANDSCAPE_TOOL_RAIL_WIDTH: f32 = 72.0;
    const MASK_THUMBNAIL_EDGE: u32 = 64;
    pub(crate) const VERTICAL_MASK_STRIP_HEIGHT: f32 = 92.0;
    pub(crate) const HORIZONTAL_MASK_STRIP_WIDTH: f32 = 92.0;
    const CONTEXT_TAB_WIDTH: f32 = 64.0;
}

mod ai_update;
mod crop;
mod develop;
mod export;
mod info;
mod inpainting;
mod look_lut;
mod masks;
mod navigation;
mod portrait_fill;

pub(crate) use export::export_settings_controls;
pub(crate) use mask_effects::effect_card_open;
