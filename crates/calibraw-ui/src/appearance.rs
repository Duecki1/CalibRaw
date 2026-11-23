//! Persisted appearance settings.

use crate::i18n::tr;
use crate::platform::PlatformCaps;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// How tightly panels, cards and sliders are packed. Serde names are
/// persisted in settings files. Control heights follow the input method, not
/// this setting.
pub(crate) enum UiDensity {
    Comfortable,
    Compact,
}

impl UiDensity {
    pub(crate) const ALL: [Self; 2] = [Self::Comfortable, Self::Compact];

    /// Compact on mobile, where the panel shares a small screen with the
    /// photo; comfortable elsewhere.
    pub(crate) const fn default_for(caps: PlatformCaps) -> Self {
        if caps.is_mobile() {
            Self::Compact
        } else {
            Self::Comfortable
        }
    }

    /// The Moduwu metrics for this density with the device's input method.
    pub(crate) const fn metrics(self, caps: PlatformCaps) -> moduwu_design::Metrics {
        let density = match self {
            Self::Comfortable => moduwu_design::Density::Comfortable,
            Self::Compact => moduwu_design::Density::Compact,
        };
        moduwu_design::Metrics::new(caps.is_mobile(), density)
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Comfortable => tr("Comfortable"),
            Self::Compact => tr("Compact"),
        }
    }
}

impl Default for UiDensity {
    fn default() -> Self {
        Self::default_for(PlatformCaps::CURRENT)
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

    /// The backdrop after this one in [`Self::ALL`], wrapping to the first.
    pub(crate) fn next(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|backdrop| *backdrop == self)
            .unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

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
