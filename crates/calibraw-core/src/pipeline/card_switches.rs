//! The switches on sidebar cards, which turn a card's edits off without
//! clearing them.
//!
//! A card that is off keeps its settings in the edit, which saves, undoes and
//! compares them like any other value, but they render at rest. Every
//! renderer draws an edit through the same projection: [`ExposureParams::applied`]
//! for the photo-wide cards, [`MaskStack::apply_card_switches`] for the
//! masks' cards and the subject refinement, and [`ExposureParams::card_on`]
//! for the LUT and Camera match looks, which the edit keeps beside these
//! settings. The Lens Corrections card's switch is the lens correction's own
//! `enabled`, which also gates the card's chromatic aberration removal.
//!
//! Switch states belong to their photo: presets and pastes never carry them.
//! Effects and masks have switched on and off through their own `enabled`
//! since before these switches, and keep doing so.

use super::{
    AdjustmentGroup, AdjustmentGroupSet, ExposureParams, FilmSource, LocalMask, MaskStack,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A photo-wide Develop card with a switch kept in [`ExposureParams::cards_off`].
/// Lens Corrections has its own (`LensEditState::enabled`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CardSwitch {
    Light,
    ToneCurve,
    Color,
    ColorGrading,
    Detail,
    Effects,
    ColorMixer,
    Calibration,
    FilmNegative,
    Lut,
    CameraMatch,
}

impl CardSwitch {
    pub const ALL: [Self; 11] = [
        Self::Light,
        Self::ToneCurve,
        Self::Color,
        Self::ColorGrading,
        Self::Detail,
        Self::Effects,
        Self::ColorMixer,
        Self::Calibration,
        Self::FilmNegative,
        Self::Lut,
        Self::CameraMatch,
    ];

    /// The card that owns `group`.
    pub const fn of_group(group: AdjustmentGroup) -> Self {
        match group {
            AdjustmentGroup::Light => Self::Light,
            AdjustmentGroup::ToneCurve => Self::ToneCurve,
            AdjustmentGroup::Color => Self::Color,
            AdjustmentGroup::ColorGrading => Self::ColorGrading,
            AdjustmentGroup::Detail => Self::Detail,
            AdjustmentGroup::Effects => Self::Effects,
            AdjustmentGroup::ColorMixer => Self::ColorMixer,
            AdjustmentGroup::Calibration => Self::Calibration,
        }
    }

    const fn group(self) -> Option<AdjustmentGroup> {
        Some(match self {
            Self::Light => AdjustmentGroup::Light,
            Self::ToneCurve => AdjustmentGroup::ToneCurve,
            Self::Color => AdjustmentGroup::Color,
            Self::ColorGrading => AdjustmentGroup::ColorGrading,
            Self::Detail => AdjustmentGroup::Detail,
            Self::Effects => AdjustmentGroup::Effects,
            Self::ColorMixer => AdjustmentGroup::ColorMixer,
            Self::Calibration => AdjustmentGroup::Calibration,
            Self::FilmNegative | Self::Lut | Self::CameraMatch => return None,
        })
    }

    const fn bit(self) -> u16 {
        1 << self as u16
    }
}

/// A set of [`CardSwitch`]es. Serialized as a list of card names, so files
/// stay readable and independent of the bit layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CardSwitches(u16);

impl CardSwitches {
    pub const EMPTY: Self = Self(0);

    pub const fn contains(self, card: CardSwitch) -> bool {
        self.0 & card.bit() != 0
    }

    /// Takes a reference for `skip_serializing_if`.
    pub const fn is_empty(&self) -> bool {
        self.0 == 0
    }

    pub fn set(&mut self, card: CardSwitch, included: bool) {
        if included {
            self.0 |= card.bit();
        } else {
            self.0 &= !card.bit();
        }
    }

    pub fn iter(self) -> impl Iterator<Item = CardSwitch> {
        CardSwitch::ALL
            .into_iter()
            .filter(move |card| self.contains(*card))
    }
}

impl Serialize for CardSwitches {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter())
    }
}

impl<'de> Deserialize<'de> for CardSwitches {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut set = Self::EMPTY;
        for card in Vec::<CardSwitch>::deserialize(deserializer)? {
            set.set(card, true);
        }
        Ok(set)
    }
}

/// `skip_serializing_if` for a mask's [`LocalMask::cards_off`].
pub(crate) const fn no_groups(groups: &AdjustmentGroupSet) -> bool {
    groups.is_empty()
}

impl ExposureParams {
    pub fn card_on(&self, card: CardSwitch) -> bool {
        !self.cards_off.contains(card)
    }

    /// Switches `card` on or off; returns whether that changed it.
    pub fn set_card_on(&mut self, card: CardSwitch, on: bool) -> bool {
        let was_on = self.card_on(card);
        self.cards_off.set(card, !on);
        was_on != on
    }

    /// The settings as they render: those of every card switched off at
    /// their defaults, without sharpening for Detail, and a film negative
    /// developed as the scan it is. Chromatic aberration removal belongs to
    /// the Lens Corrections card and is off unless `lens_card_on`, that
    /// card's switch (`LensEditState::enabled`). The result has every card
    /// on, so applying it again changes nothing.
    pub fn applied(&self, lens_card_on: bool) -> Self {
        let mut applied = *self;
        applied.cards_off = CardSwitches::EMPTY;
        for card in self.cards_off.iter() {
            match CardSwitch::group(card) {
                Some(group) => {
                    applied.reset_group(group);
                    if group == AdjustmentGroup::Detail {
                        // The default edit sharpens; an unsharpened photo is
                        // what Detail adds nothing to.
                        applied.sharpen_amount = 0.0;
                    }
                }
                None if card == CardSwitch::FilmNegative => applied.film = FilmSource::Positive,
                // Their looks sit beside these settings (`Self::card_on`).
                None => {}
            }
        }
        if !lens_card_on {
            applied.chromatic_aberration = 0.0;
        }
        applied
    }
}

impl LocalMask {
    pub fn card_on(&self, group: AdjustmentGroup) -> bool {
        !self.cards_off.contains(group)
    }

    /// Switches the mask's `group` card on or off; returns whether that
    /// changed it.
    pub fn set_card_on(&mut self, group: AdjustmentGroup, on: bool) -> bool {
        let was_on = self.card_on(group);
        self.cards_off.set(group, !on);
        was_on != on
    }
}

impl MaskStack {
    /// Whether a mask's card or the subject refinement is switched off.
    pub fn has_cards_off(&self) -> bool {
        !self.subject_refinement.enabled || self.masks.iter().any(|mask| !mask.cards_off.is_empty())
    }

    /// Puts every mask card that is switched off at rest and clears a
    /// subject refinement that is, for rendering; afterwards every card is
    /// on. Mask sources and components are left as they are.
    pub fn apply_card_switches(&mut self) {
        for mask in &mut self.masks {
            for group in mask.cards_off.iter() {
                mask.adjustments.reset_group(group);
            }
            mask.cards_off = AdjustmentGroupSet::EMPTY;
        }
        if !self.subject_refinement.enabled {
            self.subject_refinement.clear();
            self.subject_refinement.enabled = true;
        }
    }

    /// Switches every mask card and the subject refinement on, for masks a
    /// preset or paste brings: switch states stay with their photo.
    pub fn turn_cards_on(&mut self) {
        for mask in &mut self.masks {
            mask.cards_off = AdjustmentGroupSet::EMPTY;
        }
        self.subject_refinement.enabled = true;
    }
}
