use eframe::egui::Vec2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScreenLayout {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResponsiveWidth {
    Compact,
    Medium,
    Wide,
}

impl ResponsiveWidth {
    pub(crate) const COMPACT_MAX_WIDTH: f32 = 520.0;
    pub(crate) const WIDE_MIN_WIDTH: f32 = 820.0;

    pub(crate) fn from_width(width: f32) -> Self {
        if width < Self::COMPACT_MAX_WIDTH {
            Self::Compact
        } else if width < Self::WIDE_MIN_WIDTH {
            Self::Medium
        } else {
            Self::Wide
        }
    }

    pub(crate) const fn is_compact(self) -> bool {
        matches!(self, Self::Compact)
    }

    pub(crate) const fn is_wide(self) -> bool {
        matches!(self, Self::Wide)
    }
}

impl ScreenLayout {
    pub(crate) const MIN_HORIZONTAL_SIDEBAR_WIDTH: f32 = 320.0;
    #[cfg(not(target_os = "android"))]
    pub(crate) const MAX_HORIZONTAL_SIDEBAR_WIDTH: f32 =
        ResponsiveWidth::COMPACT_MAX_WIDTH;
    pub(crate) const MIN_VERTICAL_SIDEBAR_HEIGHT: f32 = 240.0;

    pub(crate) fn from_size(size: Vec2) -> Self {
        if size.x >= size.y {
            Self::Horizontal
        } else {
            Self::Vertical
        }
    }

    pub(crate) fn sidebar_default_size(self, viewport: Vec2) -> f32 {
        self.sidebar_default_size_for_platform(viewport, cfg!(target_os = "android"))
    }

    fn sidebar_default_size_for_platform(self, viewport: Vec2, android: bool) -> f32 {
        match self {
            Self::Horizontal => {
                (viewport.x * 0.28).clamp(Self::MIN_HORIZONTAL_SIDEBAR_WIDTH, 460.0)
            }
            Self::Vertical if android => {
                (viewport.y * 0.42).clamp(Self::MIN_VERTICAL_SIDEBAR_HEIGHT, 480.0)
            }
            Self::Vertical => (viewport.y * 0.40).clamp(Self::MIN_VERTICAL_SIDEBAR_HEIGHT, 480.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ResponsiveWidth, ScreenLayout};
    use eframe::egui::vec2;

    #[test]
    fn responsive_width_uses_shared_compact_medium_and_wide_breakpoints() {
        assert_eq!(
            ResponsiveWidth::from_width(319.0),
            ResponsiveWidth::Compact
        );
        assert_eq!(
            ResponsiveWidth::from_width(519.0),
            ResponsiveWidth::Compact
        );
        assert_eq!(
            ResponsiveWidth::from_width(520.0),
            ResponsiveWidth::Medium
        );
        assert_eq!(
            ResponsiveWidth::from_width(819.0),
            ResponsiveWidth::Medium
        );
        assert_eq!(
            ResponsiveWidth::from_width(820.0),
            ResponsiveWidth::Wide
        );
        assert_eq!(
            ResponsiveWidth::from_width(1200.0),
            ResponsiveWidth::Wide
        );
    }

    #[test]
    fn android_portrait_editor_preserves_preview_room() {
        let viewport = vec2(411.0, 891.0);
        let size = ScreenLayout::Vertical.sidebar_default_size_for_platform(viewport, true);
        assert_eq!(size, viewport.y * 0.42);
        assert!(size > ScreenLayout::Vertical.sidebar_default_size_for_platform(viewport, false));
        assert!(size < viewport.y * 0.5);
    }

    #[test]
    fn landscape_sidebar_size_is_platform_neutral() {
        let viewport = vec2(891.0, 411.0);
        assert_eq!(
            ScreenLayout::Horizontal.sidebar_default_size_for_platform(viewport, true),
            ScreenLayout::Horizontal.sidebar_default_size_for_platform(viewport, false)
        );
    }
}
