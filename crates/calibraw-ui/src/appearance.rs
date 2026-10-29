//! Persisted appearance settings.

use crate::i18n::tr;
use eframe::egui::Color32;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// The application colour theme. Serde names are persisted in settings files;
/// the first two keep the names of retired themes they replaced.
pub(crate) enum UiDesign {
    #[serde(rename = "midnight_pink")]
    ObsidianBlue,
    #[serde(rename = "graphite_mint")]
    ObsidianRed,
    #[default]
    PlainGreyDark,
    Porcelain,
    DaylightBlue,
    PlainGreyLight,
}

impl UiDesign {
    /// Dropdown order: the default Plain Grey designs first.
    pub(crate) const ALL: [Self; 6] = [
        Self::PlainGreyDark,
        Self::PlainGreyLight,
        Self::ObsidianBlue,
        Self::ObsidianRed,
        Self::Porcelain,
        Self::DaylightBlue,
    ];

    /// The Moduwu preset this setting selects.
    pub(crate) const fn design(self) -> moduwu_design::Design {
        use moduwu_design::Design;
        match self {
            Self::ObsidianBlue => Design::ObsidianBlue,
            Self::ObsidianRed => Design::ObsidianRed,
            Self::PlainGreyDark => Design::PlainGreyDark,
            Self::Porcelain => Design::Porcelain,
            Self::DaylightBlue => Design::DaylightBlue,
            Self::PlainGreyLight => Design::PlainGreyLight,
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        self.design().label()
    }

    pub(crate) const fn description(self) -> &'static str {
        self.design().description()
    }

    pub(crate) const fn is_dark(self) -> bool {
        self.design().is_dark()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// What surrounds the photo in the preview.
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

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Black => tr("Black"),
            Self::DarkGrey => tr("Dark grey"),
            Self::LightGrey => tr("Light grey"),
            Self::White => tr("White"),
            Self::MatchPhoto => tr("Match photo"),
        }
    }

    pub(crate) const fn color(self, adaptive: Color32) -> Color32 {
        match self {
            Self::Black => Color32::BLACK,
            Self::DarkGrey => crate::ui::theme::CANVAS_BACKDROP,
            Self::LightGrey => Color32::from_gray(168),
            Self::White => Color32::WHITE,
            Self::MatchPhoto => adaptive,
        }
    }
}
