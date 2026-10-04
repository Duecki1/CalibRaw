//! Persisted appearance settings.

use eframe::egui::Color32;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// The application colour theme. Serde names are persisted in settings files;
/// the first two keep the names of retired themes they replaced.
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

    /// The Moduwu preset this setting selects.
    pub(crate) const fn design(self) -> moduwu_design::Design {
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
            Self::DarkGrey => crate::ui::theme::CANVAS_BACKDROP,
            Self::LightGrey => Color32::from_gray(168),
            Self::White => Color32::WHITE,
            Self::MatchPhoto => adaptive,
        }
    }
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

        let blue = UiDesign::ObsidianBlue.design().palette().accent;
        let red = UiDesign::ObsidianRed.design().palette().accent;
        assert!(blue.b() > blue.r() && blue.b() > blue.g());
        assert!(red.r() > red.g() && red.r() > red.b());
        assert_ne!(blue, red);
    }

    #[test]
    fn backdrop_serde_names_are_stable() {
        for (backdrop, name) in [
            (PreviewBackdrop::Black, "black"),
            (PreviewBackdrop::DarkGrey, "dark_grey"),
            (PreviewBackdrop::LightGrey, "light_grey"),
            (PreviewBackdrop::White, "white"),
            (PreviewBackdrop::MatchPhoto, "match_photo"),
        ] {
            assert_eq!(
                serde_json::to_string(&backdrop).unwrap(),
                format!("\"{name}\"")
            );
        }
        assert_eq!(
            serde_json::to_string(&UiDesign::Porcelain).unwrap(),
            r#""porcelain""#
        );
        assert_eq!(
            serde_json::to_string(&UiDesign::DaylightBlue).unwrap(),
            r#""daylight_blue""#
        );
    }
}
