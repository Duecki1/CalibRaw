//! The order of the Develop cards, a user preference saved with the app
//! settings. The wide (sidebar) layout shows the cards in this order; the
//! vertical layout shows one section at a time and orders its tabs the same
//! way (`AdjustmentSection::of_card`).
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DevelopCard {
    CameraProfile,
    Light,
    ToneCurve,
    Lut,
    Color,
    ColorGrading,
    Detail,
    Effects,
    ColorMixer,
    LensCorrections,
}

impl DevelopCard {
    pub(crate) const DEFAULT_ORDER: [Self; 10] = [
        Self::CameraProfile,
        Self::Light,
        Self::ToneCurve,
        Self::Lut,
        Self::Color,
        Self::ColorGrading,
        Self::Detail,
        Self::Effects,
        Self::ColorMixer,
        Self::LensCorrections,
    ];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::CameraProfile => "Camera profile",
            Self::Light => "Light",
            Self::ToneCurve => "Tone Curve",
            Self::Lut => "LUT",
            Self::Color => "Color",
            Self::ColorGrading => "Color Grading",
            Self::Detail => "Detail",
            Self::Effects => "Effects",
            Self::ColorMixer => "Color Mixer",
            Self::LensCorrections => "Lens Corrections",
        }
    }

    /// The name the settings file stores. Never change one.
    const fn key(self) -> &'static str {
        match self {
            Self::CameraProfile => "camera_profile",
            Self::Light => "light",
            Self::ToneCurve => "tone_curve",
            Self::Lut => "lut",
            Self::Color => "color",
            Self::ColorGrading => "color_grading",
            Self::Detail => "detail",
            Self::Effects => "effects",
            Self::ColorMixer => "color_mixer",
            Self::LensCorrections => "lens_corrections",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::DEFAULT_ORDER
            .into_iter()
            .find(|card| card.key() == key)
    }
}

/// Every [`DevelopCard`] exactly once, in the order the sidebar shows them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DevelopCardOrder(Vec<DevelopCard>);

impl Default for DevelopCardOrder {
    fn default() -> Self {
        Self(DevelopCard::DEFAULT_ORDER.to_vec())
    }
}

impl DevelopCardOrder {
    /// `cards` without repeats, with each card it lacks placed after the
    /// card that precedes it in the default order, so a card added in a
    /// later version keeps its usual neighbour.
    fn from_cards(cards: impl IntoIterator<Item = DevelopCard>) -> Self {
        let mut order: Vec<DevelopCard> = Vec::with_capacity(DevelopCard::DEFAULT_ORDER.len());
        for card in cards {
            if !order.contains(&card) {
                order.push(card);
            }
        }
        for (index, card) in DevelopCard::DEFAULT_ORDER.into_iter().enumerate() {
            if order.contains(&card) {
                continue;
            }
            let position = DevelopCard::DEFAULT_ORDER[..index]
                .iter()
                .rev()
                .find_map(|previous| order.iter().position(|placed| placed == previous))
                .map_or(0, |previous| previous + 1);
            order.insert(position, card);
        }
        Self(order)
    }

    pub(crate) fn cards(&self) -> &[DevelopCard] {
        &self.0
    }

    pub(crate) fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Swaps the card at `index` with its neighbour above (`up`) or below.
    /// Returns whether the order changed.
    pub(crate) fn move_card(&mut self, index: usize, up: bool) -> bool {
        let Some(other) = (if up {
            index.checked_sub(1)
        } else {
            index.checked_add(1)
        }) else {
            return false;
        };
        if index >= self.0.len() || other >= self.0.len() {
            return false;
        }
        self.0.swap(index, other);
        true
    }
}

impl Serialize for DevelopCardOrder {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.iter().map(|card| card.key()))
    }
}

impl<'de> Deserialize<'de> for DevelopCardOrder {
    /// Names this version does not know, such as a card of a later version,
    /// are skipped rather than failing the whole settings file.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let keys = Vec::<String>::deserialize(deserializer)?;
        Ok(Self::from_cards(
            keys.iter().filter_map(|key| DevelopCard::from_key(key)),
        ))
    }
}
